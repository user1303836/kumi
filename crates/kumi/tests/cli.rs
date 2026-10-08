//! Native CLI dispatch and session binding: production stores, I/O and model controller.
use kumi::{
    cli::{help, run, AbletonFactory, CliIo},
    input::{ByteListener, TerminalInput},
    tui::tty::TtyOutput,
};
use kumi_runtime::{create_inference_only_integration, system::Env};
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
    Rc::new(|_| panic!("this command must not create an Ableton integration"))
}
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}
case!(help_version_errors_and_checkout_commands, async {
    let f = Fixture::new(false);
    assert_eq!(run(f.io(&["--version"]), unused_factory()).await, 0);
    assert_eq!(f.output(), format!("Kumi {}\n", kumi_runtime::KUMI_VERSION));
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
    assert_eq!(run(f.io(&["--unknown"]), unused_factory()).await, 1);
    assert!(f.err.0.borrow().starts_with("Kumi:"));
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
    use kumi_runtime::auth::store::{open_credential_store, CredentialStore};
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
    f.settings(json!({"model":"anthropic/claude-sonnet-5-5","updateCheck":false,"libraryFolders":[]}));
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
    f.settings(json!({"model":"anthropic/claude-sonnet-5-5","updateCheck":false}));
    let called = Rc::new(Cell::new(false));
    let mark = called.clone();
    let judged = Rc::new(RefCell::new(None));
    let tell = judged.clone();
    let factory: AbletonFactory = Rc::new(move |options| {
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
    });
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
fn judged_round() -> kumi_runtime::listening::round::Round {
    use kumi_runtime::listening::round::{Round, RoundKind};
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
    f.settings(json!({"model":"anthropic/claude-sonnet-5-5","updateCheck":false,"libraryFolders":[]}));
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
