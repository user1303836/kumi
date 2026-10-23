//! `kumi doctor`, and voice readiness checks.
#[path = "../support/live_words.rs"]
mod live_words;
use futures::FutureExt;
use kumi::{doctor::*, tui::tty::TtyOutput};
use kumi_ai::providers::{
    local::{local_servers, ServerSetting},
    models::ModelInfo,
    table::{add_test_rows, ProviderRow, API_KEY_ROW},
};
use kumi_common::time::now_ms;
use kumi_daw::{
    check::{Bundled, Check, CheckStatus, Probe, Probes},
    io::Answers,
};
use kumi_daw_ableton::app::answers::{ExtensionHost, Hands, LiveRunning, Lives, NodeVersions, PortOwners};
use kumi_daw_ableton::app::{
    doctor::{missing_bridge, older_bridge},
    folders::read_bridge_server,
    live_app::InstalledLive,
    live_extension::install_extension,
};
use kumi_media::voice::VoiceReadiness;
use kumi_types::{
    command::{KUMI, KUMI_REPAIR},
    system::Env,
};
use serde_json::json;
use std::{cell::RefCell, fs, path::PathBuf, rc::Rc};
#[derive(Default)]
struct Out(RefCell<String>);
impl TtyOutput for Out {
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
        self.0.borrow_mut().push_str(s)
    }
}
fn ready() -> VoiceReadiness {
    serde_json::from_value(
        json!({"ffmpeg":"/usr/bin/ffmpeg","whisper":"/usr/bin/whisper-cli","model":{"name":"ggml-small.en-q5_1.bin"},"fetches":false}),
    )
    .unwrap()
}
struct Setup {
    root: tempfile::TempDir,
    env: Env,
    package: PathBuf,
    config: PathBuf,
}
impl Setup {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let scripts = root.path().join("Remote Scripts");
        let package = root.path().join("bridge/node_modules/@ableton-mcp/mcp-server");
        fs::create_dir_all(scripts.join("AbletonMcpBridge")).unwrap();
        fs::create_dir_all(package.join("dist/src")).unwrap();
        fs::write(package.join("package.json"), json!({"version":"1.0.9"}).to_string()).unwrap();
        let node = root.path().join("_npx/abc/node");
        fs::create_dir_all(node.parent().unwrap()).unwrap();
        fs::write(&node, "#!/bin/sh\necho v24.1.0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let config = root.path().join("bridge-config.json");
        fs::write(
            &config,
            json!({"version":2,"server":{"command":node,"args":[package.join("dist/src/cli.js"),"--config",config]}}).to_string(),
        )
        .unwrap();
        fs::write(scripts.join("AbletonMcpBridge/bridge-reference.json"), json!({"config":config}).to_string()).unwrap();
        let env = Env::from([
            ("KUMI_REMOTE_SCRIPTS_DIR".into(), scripts.display().to_string()),
            ("KUMI_MODEL".into(), "openai/gpt-fixture".into()),
            ("OPENAI_API_KEY".into(), "sk-fixture-never-printed".into()),
            ("KUMI_AUTH_FILE".into(), root.path().join("auth.json").display().to_string()),
            ("KUMI_SETTINGS_FILE".into(), root.path().join("settings.json").display().to_string()),
            ("KUMI_PROJECTS_DIR".into(), root.path().join("projects").display().to_string()),
            ("KUMI_LIBRARY_DIR".into(), root.path().join("library").display().to_string()),
            ("KUMI_LIVE_EXTENSIONS_DIR".into(), root.path().join("Ableton/Extensions").display().to_string()),
            ("KUMI_HOME".into(), root.path().join(".kumi").display().to_string()),
            ("HOME".into(), root.path().display().to_string()),
        ]);
        Self { root, env, package, config }
    }
    fn io(&self) -> DoctorIo {
        let mut io = DoctorIo::new(Rc::new(Out::default()), self.env.clone());
        io.node_version = Some("v24.21.0".into());
        io.terminal = Some(TerminalInfo { is_tty: true, columns: Some(120), rows: Some(36) });
        io.answers = io.answers.clone().with(Probes(Rc::new(|_, _| {
            async {
                Ok(Probe {
                    started: true,
                    connected: Some(true),
                    set: Some("Night Drive".into()),
                    real_live: Some(true),
                    ..Default::default()
                })
            }
            .boxed_local()
        })));
        io.answers = io.answers.clone().with(NodeVersions(Rc::new(|_| async { Some("v24.1.0".into()) }.boxed_local())));
        io.video_programs = Some(Rc::new(|| {
            async { Ok(VideoPrograms { ffmpeg: Some("/usr/bin/ffmpeg".into()), whisper: Some("/usr/bin/whisper-cli".into()) }) }
                .boxed_local()
        }));
        io.answers = io.answers.clone().with(Hands(Rc::new(|| async { Ok(None) }.boxed_local())));
        io.voice = Some(Rc::new(|| async { Ok(ready()) }.boxed_local()));
        io.model_servers = Some(Rc::new(|| async { Ok(vec![]) }.boxed_local()));
        io.installation = quiet_installation();
        io
    }
}
/// The Installation section's look at this computer, answered: a Live 12, room on the disk, the ports free.
fn quiet_installation() -> InstallationIo {
    InstallationIo {
        platform: Some("darwin".into()),
        free: Some(Rc::new(|_| async { Some(50e9) }.boxed_local())),
        now: Some(now_ms() as f64),
        answers: Answers::default()
            .with(Lives(Rc::new(|| {
                vec![InstalledLive {
                    app: "/Applications/Ableton Live 12 Suite.app".into(),
                    name: "Ableton Live 12 Suite".into(),
                    version: Some("12.4.15".into()),
                    opened: true,
                }]
            })))
            .with(PortOwners(Rc::new(|_| async { None }.boxed_local())))
            .with(LiveRunning(Rc::new(|| async { false }.boxed_local())))
            .with(ExtensionHost(Rc::new(|| async { false }.boxed_local()))),
    }
}
fn find<'a>(checks: &'a [Check], text: &str) -> &'a Check {
    checks.iter().find(|c| c.text.contains(text)).unwrap_or_else(|| panic!("No {text:?} in {checks:?}"))
}
fn probe(io: &mut DoctorIo, live: Probe) {
    io.answers = io.answers.clone().with(Probes(Rc::new(move |_, _| {
        let live = live.clone();
        async move { Ok(live) }.boxed_local()
    })));
}
#[tokio::test(flavor = "current_thread")]
async fn doctor_says_whats_fine_and_exact_fixes_without_secrets() {
    let s = Setup::new();
    let out = Rc::new(Out::default());
    let mut io = s.io();
    io.out = out.clone();
    io.node_version = Some("v20.11.0".into());
    io.answers = io.answers.clone().with(Bundled(Rc::new(|_| Some("1.0.10".into()))));
    assert_eq!(run_doctor(io.clone()).await.unwrap(), 1);
    let printed = out.0.borrow().clone();
    // An older bridge in Live: the one way Kumi says to connect it, which can restart Live for the producer. Where Live
    // doesn't run (Linux), a note (older_bridge's test has its words).
    let live = kumi_daw_ableton::app::live_app::runs_on(kumi_types::system::platform());
    let older = find(&doctor_checks(&io).await.unwrap(), "The installed bridge (1.0.9) is older than this Kumi's (1.0.10)").clone();
    if live {
        assert_eq!(older.next.as_deref(), Some(kumi_types::command::CONNECT_LIVE.as_str()));
    } else {
        assert_eq!(older.status, CheckStatus::Note);
    }
    for text in [
        "Node.js 20.11.0 isn't supported (Kumi needs 22 or newer)",
        "Install Node 24 LTS",
        "openai API key from OPENAI_API_KEY · model openai/gpt-fixture",
        "The installed bridge (1.0.9) is older than this Kumi's (1.0.10)",
        "Other MCP apps would start the bridge with a Node from a temporary folder",
        "Live connected · Night Drive",
        if live { "2 things to fix" } else { "1 thing to fix" },
    ] {
        assert!(printed.contains(text), "{text}: {printed}")
    }
    assert!(!printed.contains("sk-fixture-never-printed"));
    io.node_version = Some("v25.9.0".into());
    assert!(format_doctor(&doctor_checks(&io).await.unwrap()).contains("Node.js 25.9.0 (Kumi is tested on 22 and 24)"));
}
#[tokio::test(flavor = "current_thread")]
async fn doctor_explains_live_bridge_and_terminal_in_plain_words() {
    let s = Setup::new();
    let mut io = s.io();
    probe(&mut io, Probe { started: true, connected: Some(false), ..Default::default() });
    assert_eq!(find(&doctor_checks(&io).await.unwrap(), "Live isn't connected").status, CheckStatus::Fix);
    probe(&mut io, Probe::default());
    assert!(find(&doctor_checks(&io).await.unwrap(), "didn't start").next.as_ref().unwrap().contains(KUMI_REPAIR.as_str()));
    io.answers = io.answers.clone().with(Bundled(Rc::new(|_| Some("1.0.9".into()))));
    let checks = doctor_checks(&io).await.unwrap();
    assert!(find(&checks, "couldn't reach Live").next.as_ref().unwrap().contains("answer it first"));
    assert!(!checks.iter().any(|c| c.text.contains("didn't start")));
    // Live still runs the bridge it loaded before an update (the installed one is current): restart Live, never the
    // Control Surface advice.
    let said = format!("Kumi's bridge didn't start: {}", kumi_common::bridge::ANOTHER_BRIDGE);
    probe(&mut io, Probe { said: Some(said), ..Default::default() });
    let checks = doctor_checks(&io).await.unwrap();
    let restart = find(&checks, "Live is running another version of Kumi's bridge");
    assert_eq!(restart.status, CheckStatus::Fix);
    assert!(restart.next.as_ref().unwrap().starts_with("Restart Live"), "{:?}", restart.next);
    assert!(!checks.iter().any(|c| c.text.contains("couldn't reach Live")));
    io.terminal = Some(TerminalInfo { is_tty: true, columns: Some(50), rows: Some(12) });
    assert!(find(&doctor_checks(&io).await.unwrap(), "Terminal").text.contains("small"));
    io.env.insert("KUMI_REMOTE_SCRIPTS_DIR".into(), s.root.path().join("nowhere").display().to_string());
    let checks = doctor_checks(&io).await.unwrap();
    if matches!(kumi_types::system::platform(), "darwin" | "win32") {
        let missing = find(&checks, "The Ableton bridge isn't installed");
        assert_eq!(missing.next.as_deref(), Some(kumi_types::command::CONNECT_LIVE.as_str()));
        assert!(missing.next.as_ref().unwrap().contains(&format!("{} bridge", *KUMI)));
    } else {
        // No Live for Linux: nothing to fix.
        assert_eq!(find(&checks, "There's no Live for this system, so Kumi chats without it").status, CheckStatus::Note);
    }
    io.env.insert("KUMI_MODEL".into(), "openai-codex/gpt-fixture".into());
    assert_eq!(find(&doctor_checks(&io).await.unwrap(), "ChatGPT").next, Some(format!("{} login openai-codex", *KUMI)));
}
#[tokio::test(flavor = "current_thread")]
async fn doctor_video_programs_are_optional_and_have_install_hints() {
    let s = Setup::new();
    let mut io = s.io();
    assert_eq!(find(&doctor_checks(&io).await.unwrap(), "Watches videos").status, CheckStatus::Ok);
    io.video_programs = Some(Rc::new(|| async { Ok(VideoPrograms { ffmpeg: Some("f".into()), whisper: None }) }.boxed_local()));
    let checks = doctor_checks(&io).await.unwrap();
    let note = find(&checks, "without captions needs whisper.cpp");
    assert_eq!(note.status, CheckStatus::Note);
    assert!(note.next.as_ref().unwrap().contains("whisper"));
    io.video_programs = Some(Rc::new(|| async { Ok(VideoPrograms::default()) }.boxed_local()));
    let checks = doctor_checks(&io).await.unwrap();
    assert!(find(&checks, "can't see its frames").next.as_ref().unwrap().contains("ffmpeg"));
    assert!(!checks.iter().any(|c| c.status == CheckStatus::Fix && c.text.contains("video")));
}
fn extension(folder: &std::path::Path, code: &str) {
    fs::create_dir_all(folder.join("dist")).unwrap();
    fs::write(folder.join("manifest.json"), json!({"version":"1.0.0"}).to_string()).unwrap();
    fs::write(folder.join("dist/extension.js"), code).unwrap();
}
#[tokio::test(flavor = "current_thread")]
async fn doctor_extension_is_installed_matches_the_bridge_and_answers() {
    use tokio::io::AsyncWriteExt;
    let s = Setup::new();
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        loop {
            let (mut client, _) = socket.accept().await.unwrap();
            let _ = client.write_all(b"{\"version\":\"ableton-loopback/v1\",\"id\":\"hello\",\"ok\":true}\n").await;
        }
    });
    let mut io = s.io();
    assert!(!doctor_checks(&io).await.unwrap().iter().any(|c| c.text.contains("extension")));
    let source = s.package.join("live-extension");
    extension(&source, "module.exports = {};\n");
    assert_eq!(
        find(&doctor_checks(&io).await.unwrap(), "extension").text,
        "Kumi's extension isn't in Live (it renders tracks without playing them and writes MIDI clips in the Arrangement)"
    );
    fs::create_dir_all(s.root.path().join("Ableton")).unwrap();
    install_extension(source.to_str().unwrap(), &s.env["KUMI_LIVE_EXTENSIONS_DIR"]).unwrap();
    assert_eq!(find(&doctor_checks(&io).await.unwrap(), "extension").text, "Live hasn't started Kumi's extension");
    probe(&mut io, Probe { started: true, connected: Some(false), ..Default::default() });
    assert_eq!(find(&doctor_checks(&io).await.unwrap(), "extension").text, "Kumi's extension 1.0.0 is in Live; it starts with Live");
    let data = s.root.path().join("Ableton/Extensions Data/kumi.kumi");
    fs::create_dir_all(&data).unwrap();
    let endpoint = json!({"host":"127.0.0.1","port":port,"pid":std::process::id()}).to_string();
    fs::write(data.join("endpoint.json"), &endpoint).unwrap();
    assert_eq!(find(&doctor_checks(&io).await.unwrap(), "extension").text, "Kumi's extension is running in Live");
    fs::remove_dir_all(&data).unwrap();
    let data = s.config.parent().unwrap().join("live-extension");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("endpoint.json"), endpoint).unwrap();
    assert_eq!(
        find(&doctor_checks(&io).await.unwrap(), "extension").text,
        "Kumi's extension is running (Kumi started it: Live's Developer Mode is on)"
    );
    extension(&source, "module.exports = { newer: true };\n");
    assert_eq!(find(&doctor_checks(&io).await.unwrap(), "extension").text, "Kumi's extension in Live is from another bridge");
    probe(&mut io, Probe { started: true, live_version: Some("12.3.2".into()), ..Default::default() });
    assert!(find(&doctor_checks(&io).await.unwrap(), "extension").text.contains("Live 12.3.2 runs no extensions (12.4 and later do)"));
    server.abort();
}
#[tokio::test(flavor = "current_thread")]
async fn doctor_hands_names_the_permission_and_library_progress() {
    let s = Setup::new();
    let mut io = s.io();
    let off = Check::fix(
        "Kumi can't use Live's own menus until Accessibility is on for this terminal",
        Some("System Settings › Privacy & Security › Accessibility: turn on the app Kumi runs in".into()),
    );
    io.answers = io.answers.clone().with(Hands(Rc::new({
        let off = off.clone();
        move || {
            let off = off.clone();
            async move { Ok(Some(off)) }.boxed_local()
        }
    })));
    assert_eq!(*find(&doctor_checks(&io).await.unwrap(), "own menus"), off);
    assert!(!doctor_checks(&s.io()).await.unwrap().iter().any(|c| c.text.contains("own menus")));
    assert_eq!(find(&doctor_checks(&io).await.unwrap(), "library").text, "Kumi hasn't learned your library yet");
    let folder = &s.env["KUMI_LIBRARY_DIR"];
    fs::create_dir_all(folder).unwrap();
    fs::write(PathBuf::from(folder).join("state.json"),json!({"version":1,"last":{"startedAt":now_ms()-3_600_000,"finishedAt":now_ms()-3_000_000,"sounds":48210,"presets":3140,"sets":37,"failed":2}}).to_string()).unwrap();
    assert_eq!(
        find(&doctor_checks(&io).await.unwrap(), "library").text,
        "Knows your library: 48,210 sounds, 3,140 presets, 37 Sets (learned 50 minutes ago)"
    );
    fs::write(PathBuf::from(folder).join("state.json"),json!({"version":1,"learning":{"pid":std::process::id(),"startedAt":now_ms(),"phase":"sounds","sounds":{"known":1204,"todo":8311,"done":1204},"presets":{"known":0,"todo":0,"done":0},"sets":{"known":0,"todo":0,"done":0},"updatedAt":now_ms()}}).to_string()).unwrap();
    assert_eq!(
        find(&doctor_checks(&io).await.unwrap(), "library").text,
        "Learning your library in the background: 1,204 of 8,311 new sounds"
    );
}
fn servers(io: &mut DoctorIo, found: Vec<ServerFinding>) {
    io.model_servers = Some(Rc::new(move || {
        let found = found.clone();
        async move { Ok(found) }.boxed_local()
    }));
}
#[tokio::test(flavor = "current_thread")]
async fn doctor_model_servers_lists_models_and_checks_chosen_model() {
    let s = Setup::new();
    let local = local_servers(&[], &Env::new()).unwrap();
    let qwen: ModelInfo = serde_json::from_value(
        json!({"id":"ollama/qwen3:8b","provider":"ollama","model":"qwen3:8b","name":"qwen3:8b","efforts":[],"tools":true}),
    )
    .unwrap();
    let gemma = ModelInfo {
        id: "ollama/gemma3:4b".into(),
        model: "gemma3:4b".into(),
        name: "gemma3:4b".into(),
        tools: Some(false),
        ..qwen.clone()
    };
    let found = vec![
        ServerFinding { server: local[0].clone(), running: true, models: Some(vec![qwen, gemma]) },
        ServerFinding { server: local[1].clone(), running: false, models: None },
    ];
    let mut io = s.io();
    servers(&mut io, found);
    io.env.insert("KUMI_MODEL".into(), "ollama/qwen3:8b".into());
    let checks = doctor_checks(&io).await.unwrap();
    assert_eq!(checks[1], Check::ok("Ollama on this computer · model ollama/qwen3:8b"));
    assert_eq!(checks[2], Check::ok("Model servers: Ollama on this computer (2 models, 1 can change the Set)"));
    assert_eq!(
        checks[3],
        Check::note(
            "LM Studio is installed but not running",
            Some("Open LM Studio and start its server (Developer tab), or run: lms server start".into())
        )
    );
    io.env.insert("KUMI_MODEL".into(), "ollama/llama9:70b".into());
    assert_eq!(
        doctor_checks(&io).await.unwrap()[1],
        Check::fix("Ollama doesn't have llama9:70b (model ollama/llama9:70b)", Some("Run: ollama pull llama9:70b".into()))
    );
    io.env.insert("KUMI_MODEL".into(), "lmstudio/qwen/qwen3-8b".into());
    let checks = doctor_checks(&io).await.unwrap();
    assert_eq!(checks[1].text, "LM Studio isn't running (model lmstudio/qwen/qwen3-8b)");
    assert_eq!(checks.iter().filter(|c| c.text.contains("LM Studio")).count(), 1);
    io.env.remove("KUMI_MODEL");
    io.env.remove("OPENAI_API_KEY");
    assert_eq!(
        doctor_checks(&io).await.unwrap()[1].text,
        "Kumi starts with a model in Ollama, on this computer; no sign-in needed (/model changes it)"
    );
    servers(&mut io, vec![]);
    assert!(doctor_checks(&io).await.unwrap()[1].next.as_ref().unwrap().ends_with("or open Ollama or LM Studio"));
    let away = local_servers(&[], &Env::from([("OLLAMA_HOST".into(), "studio.local".into())])).unwrap().remove(0);
    servers(&mut io, vec![ServerFinding { server: away, running: false, models: None }]);
    io.env.insert("OLLAMA_HOST".into(), "studio.local".into());
    assert_eq!(
        find(&doctor_checks(&io).await.unwrap(), "Ollama isn't answering").text,
        "Ollama isn't answering at http://studio.local:11434 (OLLAMA_HOST)"
    );
}
#[test]
fn voice_readiness_is_advice_never_a_fix() {
    let model = json!({"name":"ggml-small.en-q5_1.bin","path":"/tools/ggml-small.en-q5_1.bin"});
    let cases = [
        (
            json!({"ffmpeg":"f","whisper":"w","model":model,"fetches":false}),
            Env::new(),
            "darwin",
            json!({"status":"ok","text":"Talking to Kumi (ctrl+t): ffmpeg hears the microphone, whisper.cpp writes down what you say, on this computer"}),
        ),
        (
            json!({"ffmpeg":"f","model":model,"fetches":false}),
            Env::new(),
            "darwin",
            json!({"status":"note","text":"Talking to Kumi (ctrl+t) needs whisper.cpp","next":"Install it: brew install whisper-cpp"}),
        ),
        (
            json!({"model":model,"fetches":false}),
            Env::new(),
            "darwin",
            json!({"status":"note","text":"Talking to Kumi (ctrl+t) needs ffmpeg and whisper.cpp","next":"Install them: brew install ffmpeg whisper-cpp"}),
        ),
        (
            json!({"ffmpeg":"f","whisper":"w","model":model,"fetches":false,"allowed":false}),
            Env::from([("TERM_PROGRAM".into(), "iTerm.app".into())]),
            "darwin",
            json!({"status":"note","text":"Talking to Kumi (ctrl+t): macOS isn't letting iTerm2 use the microphone","next":"Allow it in System Settings › Privacy & Security › Microphone"}),
        ),
        (
            json!({"ffmpeg":"f","whisper":"w","model":{"name":"ggml-small.en-q5_1.bin"},"fetches":false}),
            Env::new(),
            "darwin",
            json!({"status":"ok","text":"Talking to Kumi (ctrl+t): Kumi fetches its speech model (about 190 MB) the first time you talk"}),
        ),
        (
            json!({"model":{"name":"ggml-small.en-q5_1.bin"},"fetches":true}),
            Env::new(),
            "win32",
            json!({"status":"ok","text":"Talking to Kumi (ctrl+t): Kumi fetches ffmpeg, whisper.cpp and its speech model (about 190 MB) the first time you talk"}),
        ),
    ];
    for (voice, env, platform, want) in cases {
        assert_eq!(serde_json::to_value(voice_check(&serde_json::from_value(voice).unwrap(), &env, platform)).unwrap(), want)
    }
}
#[tokio::test(flavor = "current_thread")]
async fn native_bridge_configuration_resolves_binary_and_sibling_metadata() {
    let s = Setup::new();
    let binary = s.root.path().join("native/ableton-mcp-server");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, "native fixture").unwrap();
    fs::write(
        binary.parent().unwrap().join("package.json"),
        json!({"version":kumi_types::version::KUMI_VERSION,"bridge":"1.0.11"}).to_string(),
    )
    .unwrap();
    fs::write(&s.config, json!({"version":2,"server":{"command":binary,"args":["--config",s.config]}}).to_string()).unwrap();
    let server = read_bridge_server(s.config.to_str().unwrap()).unwrap();
    assert!(server.native());
    assert_eq!(server.entry, Some(binary.display().to_string()));
    assert_eq!(server.version, Some("1.0.11".into()));
    assert_eq!(server.package_root(), Some(binary.parent().unwrap().display().to_string()));
    let mut io = s.io();
    io.node_version = None;
    // A native Kumi's version heads the Installation section; there's no Node to check.
    assert!(!doctor_checks(&io).await.unwrap().iter().any(|c| c.text.contains("native executable") || c.text.contains("Node")));
    let installation = installation_checks(&io.env, &io.installation).await;
    assert!(installation[0].text.starts_with(&format!("Kumi {}", kumi_types::version::KUMI_VERSION)), "{installation:?}");
}
fn put(path: &std::path::Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
/// A file the shell can run: `kumi`, or `kumi.cmd` on Windows.
fn program(folder: &std::path::Path) -> PathBuf {
    let file = folder.join(if cfg!(windows) { "kumi.cmd" } else { "kumi" });
    put(&file, "#!/bin/sh\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    }
    file
}
fn path_of(folders: &[&std::path::Path]) -> String {
    folders.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(if cfg!(windows) { ";" } else { ":" })
}
#[tokio::test(flavor = "current_thread")]
async fn installation_names_this_kumi_what_it_goes_back_to_and_how_the_shell_finds_it() {
    let s = Setup::new();
    let mut io = s.io();
    let home = s.root.path().join(".kumi");
    io.env.insert("KUMI_INSTALLED".into(), "1".into());
    put(&home.join("app/package.json"), &json!({"version":kumi_types::version::KUMI_VERSION}).to_string());
    let checks = installation_checks(&io.env, &io.installation).await;
    assert!(checks[0].text.starts_with(&format!("Kumi {} in ", kumi_types::version::KUMI_VERSION)), "{checks:?}");
    assert_eq!(find(&checks, "Nothing kept to go back to yet").status, CheckStatus::Ok);
    put(&home.join("app.previous/package.json"), &json!({"version":"1.15.0"}).to_string());
    assert!(find(&installation_checks(&io.env, &io.installation).await, "update --rollback goes back to 1.15.0").next.is_none());
    put(&home.join("app.previous/package.json"), &json!({"version":kumi_types::version::KUMI_VERSION}).to_string());
    find(&installation_checks(&io.env, &io.installation).await, "has only this same");
    // The shell finds Kumi's launcher, or another kumi before it, or none in this window.
    let ours = home.join("bin");
    program(&ours);
    let elsewhere = s.root.path().join("old-install").join("bin");
    let other = program(&elsewhere);
    io.env.insert("PATH".into(), path_of(&[&elsewhere, &ours]));
    let first = find(&installation_checks(&io.env, &io.installation).await, "Another kumi comes first on PATH").clone();
    assert_eq!(first.status, CheckStatus::Fix);
    // Shown from the home folder as ~ where it's under it (Windows' temporary folder is).
    let shown = other.strip_prefix(s.root.path()).unwrap().display().to_string();
    assert!(first.text.ends_with(&shown) && first.next.unwrap().contains("before"), "{}", first.text);
    io.env.insert("PATH".into(), path_of(&[&ours, &elsewhere]));
    assert_eq!(find(&installation_checks(&io.env, &io.installation).await, "kumi on PATH").status, CheckStatus::Ok);
    io.env.insert("PATH".into(), path_of(&[&s.root.path().join("nowhere")]));
    let missing = find(&installation_checks(&io.env, &io.installation).await, "kumi isn't on PATH in this window").clone();
    assert_eq!(missing.status, CheckStatus::Note);
    if !cfg!(windows) {
        assert!(missing.next.unwrap().starts_with("Add "));
        put(&s.root.path().join(".zshrc"), &format!("{}\nexport PATH=\"{}:$PATH\"\n", kumi::install::PATH_MARKER, ours.display()));
        let added = find(&installation_checks(&io.env, &io.installation).await, "kumi isn't on PATH").next.clone().unwrap();
        assert!(added.starts_with("Open a new terminal window (") && added.contains(".zshrc adds it there"), "{added}");
    }
}
#[tokio::test(flavor = "current_thread")]
async fn installation_says_whether_a_newer_kumi_is_out_from_the_daily_check_without_asking() {
    let s = Setup::new();
    let io = s.io();
    // A checkout's section: this Kumi, then whether a newer one is out.
    let check = |io: DoctorIo| async move { installation_checks(&io.env, &io.installation).await.remove(1) };
    assert!(check(io.clone()).await.text.starts_with("Kumi looks for updates once a day while it runs"));
    let cache = s.root.path().join("update-check.json");
    let at = now_ms() as f64 - 3. * 3_600_000.;
    // Versions compare as numbers, from this one: x.y.(z + 10) is newer, and 1.9.0 is older than any 1.10 or later.
    let this: Vec<u32> = kumi_types::version::KUMI_VERSION.split(['.', '-']).take(3).map(|part| part.parse().unwrap()).collect();
    let later = format!("{}.{}.{}", this[0], this[1], this[2] + 10);
    for (latest, newer) in [("99.0.0", true), (later.as_str(), true), ("1.9.0", false), (kumi_types::version::KUMI_VERSION, false)] {
        fs::write(&cache, json!({"checkedAt":at,"latest":latest}).to_string()).unwrap();
        let said = check(io.clone()).await;
        if newer {
            assert_eq!(said.text, format!("Kumi {latest} is out (this is {})", kumi_types::version::KUMI_VERSION));
            assert_eq!((said.status, said.next.unwrap()), (CheckStatus::Note, format!("{} update", *KUMI)));
        } else {
            assert_eq!(said, Check::ok("Up to date when Kumi last looked (3 hours ago)"), "{latest}");
        }
    }
    fs::write(s.root.path().join("settings.json"), json!({"updateCheck":false}).to_string()).unwrap();
    assert!(check(io.clone()).await.text.contains("(updateCheck in settings.json)"));
    let mut off = io.clone();
    off.env.insert("KUMI_NO_UPDATE_CHECK".into(), "1".into());
    assert!(check(off).await.text.contains("(KUMI_NO_UPDATE_CHECK)"));
}
#[tokio::test(flavor = "current_thread")]
async fn installation_checks_room_live_its_user_library_and_the_bridges_ports() {
    let s = Setup::new();
    let mut io = s.io();
    let installation = |io: &DoctorIo| {
        let io = io.clone();
        async move { installation_checks(&io.env, &io.installation).await }
    };
    assert!(find(&installation(&io).await, "free on the disk with").text.starts_with("50.0 GB"));
    io.installation.free = Some(Rc::new(|_| async { Some(120e6) }.boxed_local()));
    let low = find(&installation(&io).await, "free on the disk with").clone();
    assert_eq!((low.status, low.text.starts_with("Only 120 MB")), (CheckStatus::Fix, true));
    assert!(low.next.unwrap().contains("250 MB"));
    io.installation = quiet_installation();
    assert_eq!(find(&installation(&io).await, "Live installed").text, "Live installed: 12.4.15 (Ableton Live 12 Suite)");
    let eleven = InstalledLive {
        app: "/Applications/Ableton Live 11 Suite.app".into(),
        name: "Ableton Live 11 Suite".into(),
        version: Some("11.3.21".into()),
        opened: true,
    };
    // Only an older Live, or none (a build machine, say): listed as it is, never a fix (the bridge's check says what to
    // do), and no User Library to look for. The exit code stays.
    let scripts = io.env.remove("KUMI_REMOTE_SCRIPTS_DIR").unwrap();
    io.installation.answers = io.installation.answers.clone().with(Lives(Rc::new(move || vec![eleven.clone()])));
    let older = installation(&io).await;
    assert_eq!(find(&older, "Live installed").text, "Live installed: 11.3.21 (Ableton Live 11 Suite)");
    assert!(!older.iter().any(|c| c.text.contains("User Library")), "{older:?}");
    io.installation.answers = io.installation.answers.clone().with(Lives(Rc::new(Vec::new)));
    let none = installation(&io).await;
    assert_eq!(find(&none, "Live isn't installed here").status, CheckStatus::Note);
    assert_eq!(doctor_code(&[Section { title: "Installation".into(), checks: none }]), 0);
    io.installation.platform = Some("linux".into());
    assert!(!installation(&io).await.iter().any(|c| c.text.contains("Live")), "nothing about Live where it doesn't run");
    // The User Library Live's settings name, there or not.
    io.installation = quiet_installation();
    let settings = PathBuf::from(kumi_daw_ableton::app::folders::live_preferences_dir(&io.env)).join("Live 12.4.15");
    let library = s.root.path().join("Music Library");
    let config = format!(
        "<Ableton><UserLibrary><ProjectPath Value=\"{}\" /><ProjectName Value=\"User Library\" /></UserLibrary></Ableton>",
        library.display()
    );
    put(&settings.join(if cfg!(windows) { "Preferences/Library.cfg" } else { "Library.cfg" }), &config);
    let gone = find(&installation(&io).await, "User Library is set to").clone();
    assert_eq!(gone.status, CheckStatus::Fix);
    assert!(gone.text.ends_with("which isn't there") && gone.next.unwrap().contains("Settings → Library"));
    fs::create_dir_all(library.join("User Library")).unwrap();
    assert_eq!(find(&installation(&io).await, "User Library:").status, CheckStatus::Ok);
    io.env.insert("KUMI_REMOTE_SCRIPTS_DIR".into(), scripts);
    assert!(find(&installation(&io).await, "Remote Scripts folder:").text.ends_with("(KUMI_REMOTE_SCRIPTS_DIR)"));
    // The bridge's ports: free, or another program's.
    assert_eq!(find(&installation(&io).await, "9765").text, "Ports 9765 and 9766 free for Live's bridge");
    io.installation.answers = io
        .installation
        .answers
        .clone()
        .with(PortOwners(Rc::new(|port| async move { (port == 9766).then(|| Some("node".into())) }.boxed_local())));
    let held = find(&installation(&io).await, "9766").clone();
    assert_eq!(held, Check::fix("node holds port 9766, which Live's bridge needs", Some("Quit node, then restart Live".into())));
}
#[tokio::test(flavor = "current_thread")]
async fn installation_tells_developer_mode_by_who_started_kumis_extension() {
    let s = Setup::new();
    let mut io = s.io();
    let mode = |io: &DoctorIo| {
        let io = io.clone();
        async move { installation_checks(&io.env, &io.installation).await.into_iter().find(|c| c.text.contains("Developer Mode")) }
    };
    assert_eq!(mode(&io).await, None, "Live closed and no extension running: can't tell");
    let endpoint = json!({"host":"127.0.0.1","port":1,"pid":std::process::id()}).to_string();
    let data = s.root.path().join("Ableton/Extensions Data/kumi.kumi");
    put(&data.join("endpoint.json"), &endpoint);
    assert!(mode(&io).await.unwrap().text.starts_with("Live's Developer Mode is off"));
    fs::remove_dir_all(&data).unwrap();
    put(&s.config.parent().unwrap().join("live-extension/endpoint.json"), &endpoint);
    assert!(mode(&io).await.unwrap().text.starts_with("Live's Developer Mode is on"));
    fs::remove_dir_all(s.config.parent().unwrap().join("live-extension")).unwrap();
    // Live open with Kumi's extension installed but not running: Live runs its own Extension Host only with it off.
    extension(&s.root.path().join("Ableton/Extensions/kumi.kumi"), "module.exports = {};\n");
    io.installation.answers = io.installation.answers.clone().with(LiveRunning(Rc::new(|| async { true }.boxed_local())));
    assert!(mode(&io).await.unwrap().text.starts_with("Live's Developer Mode is on"));
    io.installation.answers = io.installation.answers.clone().with(ExtensionHost(Rc::new(|| async { true }.boxed_local())));
    assert!(mode(&io).await.unwrap().text.starts_with("Live's Developer Mode is off"));
}
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn installation_names_a_kumi_folder_another_account_owns() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let s = Setup::new();
    let mut io = s.io();
    // /usr stands in for a ~/.kumi an install run with sudo left to root.
    io.env.insert("KUMI_HOME".into(), "/usr".into());
    let theirs = find(&installation_checks(&io.env, &io.installation).await, "another account").clone();
    assert_eq!(theirs, Check::fix("/usr belongs to another account (installed with sudo?)", Some("sudo chown -R $USER /usr".into())));
}
#[tokio::test(flavor = "current_thread")]
async fn doctor_json_takes_out_keys_and_counts_only_fixes() {
    let s = Setup::new();
    let out = Rc::new(Out::default());
    let mut io = s.io();
    io.out = out.clone();
    io.json = true;
    io.answers = io
        .answers
        .clone()
        .with(Hands(Rc::new(|| async { Ok(Some(Check::note("Read sk-fixture-never-printed in a log", None))) }.boxed_local())));
    assert_eq!(run_doctor(io.clone()).await.unwrap(), 0);
    let printed = out.0.borrow().clone();
    assert!(!printed.contains("sk-fixture-never-printed") && printed.contains("Read [secret] in a log"), "{printed}");
    let value: serde_json::Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(value["kumi"], kumi_types::version::KUMI_VERSION);
    assert_eq!(value["fixes"], 0);
    let titles: Vec<_> = value["sections"].as_array().unwrap().iter().map(|s| s["title"].as_str().unwrap().to_string()).collect();
    assert_eq!(titles, ["Installation", "Sign-in, Live and the rest"]);
    assert!(value["sections"][0]["checks"][0]["text"].as_str().unwrap().starts_with("Kumi "));
    out.0.borrow_mut().clear();
    io.answers = io.answers.clone().with(Hands(Rc::new(|| {
        async { Ok(Some(Check::fix("Kumi can't use Live's menus", Some("Run kumi report".into())))) }.boxed_local()
    })));
    assert_eq!(run_doctor(io).await.unwrap(), 1);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&out.0.borrow()).unwrap()["fixes"], 1);
}
#[test]
fn an_older_bridge_is_to_fix_where_live_runs_and_elsewhere_says_only_what_that_system_can_do() {
    for platform in ["darwin", "win32"] {
        let check = older_bridge(platform, "1.0.80", "1.0.92");
        assert_eq!(
            (check.status, check.text.as_str()),
            (CheckStatus::Fix, "The installed bridge (1.0.80) is older than this Kumi's (1.0.92)")
        );
        assert_eq!(check.next.as_deref(), Some(kumi_types::command::CONNECT_LIVE.as_str()), "{platform}");
    }
    // Linux: no Live to load it, so nothing to fix and nothing about Live; its files can be brought up to date.
    let linux = older_bridge("linux", "1.0.80", "1.0.92");
    assert_eq!(linux.status, CheckStatus::Note);
    assert_eq!(linux.next, Some(format!("{} bridge --yes brings its files up to date", *KUMI)));
    assert!(!format!("{linux:?}").contains("Live"), "{linux:?}");
}
#[test]
fn a_missing_bridge_is_to_fix_only_where_a_live_12_can_take_it() {
    use kumi_daw::provider::Installed as LiveInstalled;
    // Linux: no Live, so nothing to fix, and Live's folders aren't read at all.
    let none = missing_bridge("linux", || unreachable!("no Live folders on Linux"));
    assert_eq!(
        (none.status, none.text.as_str(), none.next),
        (CheckStatus::Note, "There's no Live for this system, so Kumi chats without it", None)
    );
    // No Live 12 here: a note, saying what Kumi works with.
    for (found, text) in [
        (LiveInstalled::Missing, "Live 12 isn't on this computer, so Kumi chats without Live"),
        (LiveInstalled::Older("11.3.21".into()), "Only Live 11.3.21 is on this computer, so Kumi chats without Live"),
    ] {
        let check = missing_bridge("darwin", || found);
        assert_eq!((check.status, check.text.as_str()), (CheckStatus::Note, text));
        assert!(check.next.unwrap().starts_with("Kumi works with Live 12: once it's installed and opened once, run: "));
    }
    // Live 12 here (opened or not): the bridge is to put in, the one way.
    for found in [LiveInstalled::Ready, LiveInstalled::NeverOpened("/Applications/Ableton Live 12 Suite.app".into())] {
        let check = missing_bridge("win32", || found);
        assert_eq!(check.status, CheckStatus::Fix);
        assert_eq!(check.next.as_deref(), Some(kumi_types::command::CONNECT_LIVE.as_str()));
    }
}

/// A producer with REAPER's bridge alone (and no Live): the doctor speaks of REAPER, never of Live, and finds nothing
/// to fix for a DAW they don't use.
#[tokio::test(flavor = "current_thread")]
async fn a_reaper_only_doctor_never_speaks_lives_words() {
    let root = tempfile::tempdir().unwrap();
    let path = |p: &str| root.path().join(p).display().to_string();
    let env = Env::from([
        ("KUMI_MODEL".into(), "openai/gpt-fixture".into()),
        ("OPENAI_API_KEY".into(), "sk-fixture-never-printed".into()),
        ("KUMI_AUTH_FILE".into(), path("auth.json")),
        ("KUMI_SETTINGS_FILE".into(), path("settings.json")),
        ("KUMI_PROJECTS_DIR".into(), path("projects")),
        ("KUMI_LIBRARY_DIR".into(), path("library")),
        ("KUMI_LIVE_EXTENSIONS_DIR".into(), path("Ableton/Extensions")),
        ("KUMI_REAPER_RESOURCE_DIR".into(), path("REAPER")),
        ("KUMI_RENOISE_PREFS_DIR".into(), path("Renoise")),
        ("KUMI_HOME".into(), path(".kumi")),
        ("HOME".into(), root.path().display().to_string()),
    ]);
    // Kumi's extension in REAPER, as its receipt says.
    let paths = daw_bridge::paths::BridgePaths::new(&root.path().join(".kumi"), daw_bridge::daw::Daw::Reaper);
    fs::create_dir_all(&paths.root).unwrap();
    fs::create_dir_all(root.path().join("REAPER/UserPlugins")).unwrap();
    let receipt = daw_bridge::lifecycle::Receipt {
        schema: daw_bridge::lifecycle::RECEIPT_SCHEMA.into(),
        daw: "reaper".into(),
        status: "installed".into(),
        target: path("REAPER/UserPlugins/reaper_kumi.dylib"),
        reference: path("REAPER/UserPlugins/kumi-bridge-reference.json"),
        generation: Some("g1".into()),
        version: Some("0.1.0".into()),
        sha256: None,
        previous: vec![],
        copies: vec![],
        updated_ms: 0,
        history: vec![],
    };
    fs::write(paths.receipt(), serde_json::to_string(&receipt).unwrap()).unwrap();
    daw_bridge::files::ensure_secret(&paths.secret()).unwrap();
    fs::write(root.path().join("REAPER/UserPlugins/reaper_kumi.dylib"), b"extension").unwrap();
    fs::write(root.path().join("REAPER/UserPlugins/kumi-bridge-reference.json"), "{}").unwrap();
    kumi::daw::work_with(kumi::daw::chosen(None, &env).0.id());
    let mut io = DoctorIo::new(Rc::new(Out::default()), env.clone());
    io.terminal = Some(TerminalInfo { is_tty: true, columns: Some(120), rows: Some(36) });
    io.video_programs = Some(Rc::new(|| {
        async { Ok(VideoPrograms { ffmpeg: Some("/usr/bin/ffmpeg".into()), whisper: Some("/usr/bin/whisper-cli".into()) }) }.boxed_local()
    }));
    io.answers = io.answers.clone().with(Hands(Rc::new(|| async { Ok(None) }.boxed_local())));
    io.voice = Some(Rc::new(|| async { Ok(ready()) }.boxed_local()));
    io.model_servers = Some(Rc::new(|| async { Ok(vec![]) }.boxed_local()));
    let mut installation = quiet_installation();
    installation.answers = installation.answers.clone().with(Lives(Rc::new(Vec::new)));
    io.installation = installation;
    let sections = doctor_sections(&io).await.unwrap();
    let text = sections
        .iter()
        .flat_map(|section| {
            std::iter::once(section.title.clone())
                .chain(section.checks.iter().map(|c| format!("{} {}", c.text, c.next.clone().unwrap_or_default())))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let leaks = live_words::leaks(&text);
    assert!(leaks.is_empty(), "the doctor says Live's words:\n{}\n\nin:\n{text}", leaks.join("\n"));
    assert!(text.contains("REAPER"), "{text}");
    let fixes: Vec<&Check> = sections.iter().flat_map(|s| &s.checks).filter(|c| c.status == CheckStatus::Fix).collect();
    assert!(fixes.iter().all(|c| c.text.contains("REAPER")), "nothing to fix for a DAW this producer doesn't use: {fixes:?}");
    kumi::daw::work_with(kumi_types::daw::DawId::Ableton);
}

/// A Live producer who has opened Renoise once (its preferences folder is there) but never put Kumi's tool in sees the
/// doctor as before REAPER and Renoise (review L7); `kumi --daw renoise doctor` gets a note about it, never a fix.
#[tokio::test(flavor = "current_thread")]
async fn a_live_producer_who_opened_renoise_once_gets_a_note_not_a_fix() {
    kumi::daw::work_with(kumi_types::daw::DawId::Ableton);
    let s = Setup::new();
    let mut io = s.io();
    io.env.insert("KUMI_REAPER_RESOURCE_DIR".into(), s.root.path().join("no-reaper").display().to_string());
    io.env.insert("KUMI_RENOISE_PREFS_DIR".into(), s.root.path().join("no-renoise").display().to_string());
    let fixes = |checks: &[Check]| checks.iter().filter(|c| c.status == CheckStatus::Fix).map(|c| c.text.clone()).collect::<Vec<_>>();
    let before = doctor_checks(&io).await.unwrap();
    let prefs = s.root.path().join("Renoise");
    fs::create_dir_all(prefs.join("V3.5.4")).unwrap();
    io.env.insert("KUMI_RENOISE_PREFS_DIR".into(), prefs.display().to_string());
    let after = doctor_checks(&io).await.unwrap();
    let texts = |checks: &[Check]| checks.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts(&before), texts(&after), "Live's doctor as before: no rows for a DAW never set up");
    assert!(!texts(&after).iter().any(|text| text.contains("Renoise")), "{:?}", texts(&after));
    // Asked about Renoise (`--daw renoise doctor`): a note, never a fix.
    kumi::daw::work_with(kumi_types::daw::DawId::Renoise);
    let asked = doctor_checks(&io).await.unwrap();
    kumi::daw::work_with(kumi_types::daw::DawId::Ableton);
    assert_eq!(fixes(&before), fixes(&asked), "no fix for a DAW never set up");
    let note = find(&asked, "Kumi's tool isn't in Renoise");
    assert_eq!(note.status, CheckStatus::Note);
    assert!(note.next.as_deref().is_some_and(|next| next.contains("--daw renoise bridge")), "{note:?}");
}

/// A KUMI_DAW Kumi doesn't know: the doctor still runs, and says that's why Kumi won't start.
#[tokio::test(flavor = "current_thread")]
async fn a_kumi_daw_kumi_doesnt_know_is_a_fix_the_doctor_names() {
    let root = tempfile::tempdir().unwrap();
    let path = |p: &str| root.path().join(p).display().to_string();
    let env = Env::from([
        ("KUMI_DAW".into(), "bitwig".into()),
        ("KUMI_MODEL".into(), "openai/gpt-fixture".into()),
        ("OPENAI_API_KEY".into(), "sk-fixture-never-printed".into()),
        ("KUMI_AUTH_FILE".into(), path("auth.json")),
        ("KUMI_SETTINGS_FILE".into(), path("settings.json")),
        ("KUMI_PROJECTS_DIR".into(), path("projects")),
        ("KUMI_LIBRARY_DIR".into(), path("library")),
        ("KUMI_LIVE_EXTENSIONS_DIR".into(), path("Ableton/Extensions")),
        ("KUMI_HOME".into(), path(".kumi")),
        ("HOME".into(), root.path().display().to_string()),
    ]);
    let mut io = DoctorIo::new(Rc::new(Out::default()), env.clone());
    io.terminal = Some(TerminalInfo { is_tty: true, columns: Some(120), rows: Some(36) });
    io.video_programs = Some(Rc::new(|| {
        async { Ok(VideoPrograms { ffmpeg: Some("/usr/bin/ffmpeg".into()), whisper: Some("/usr/bin/whisper-cli".into()) }) }.boxed_local()
    }));
    io.answers = io.answers.clone().with(Hands(Rc::new(|| async { Ok(None) }.boxed_local())));
    io.voice = Some(Rc::new(|| async { Ok(ready()) }.boxed_local()));
    io.model_servers = Some(Rc::new(|| async { Ok(vec![]) }.boxed_local()));
    io.installation = quiet_installation();
    let sections = doctor_sections(&io).await.unwrap();
    let fix = sections.iter().flat_map(|s| &s.checks).find(|c| c.text.contains("KUMI_DAW")).expect("the doctor names KUMI_DAW");
    assert_eq!(fix.status, CheckStatus::Fix);
    assert_eq!(fix.text, "Kumi won't start: KUMI_DAW must be one of: ableton, reaper, renoise.");
    assert_eq!(fix.next.as_deref(), Some("Set KUMI_DAW to one of those, or unset it"));
}
/// A provider added after the first five, as Venice would be: only this test's thread has it.
static ADDED: [ProviderRow; 1] = [ProviderRow {
    id: "venice",
    name: "Venice AI",
    short_name: "Venice",
    credential: "venice",
    key_env: &["VENICE_API_KEY"],
    base_url: "https://api.venice.ai/api/v1",
    ..API_KEY_ROW
}];
#[tokio::test(flavor = "current_thread")]
async fn doctor_names_the_custom_server_that_hides_a_provider_added_later_and_says_how_to_switch() {
    add_test_rows(&ADDED);
    let s = Setup::new();
    let setting = ServerSetting { name: "Venice".into(), base_url: "https://venice.example.test/v1".into(), api_key: None };
    fs::write(&s.env["KUMI_SETTINGS_FILE"], json!({"modelServers":[setting]}).to_string()).unwrap();
    let server = local_servers(&[setting], &Env::new()).unwrap().remove(2);
    let mut io = s.io();
    servers(&mut io, vec![ServerFinding { server, running: true, models: Some(vec![]) }]);
    io.env.insert("KUMI_MODEL".into(), "venice/qwen3-coder".into());
    let checks = doctor_checks(&io).await.unwrap();
    assert_eq!(checks[1], Check::ok("Venice on venice.example.test · model venice/qwen3-coder"));
    assert_eq!(
        find(&checks, "Your server Venice"),
        &Check::note(
            "Your server Venice, in settings.json, has the ID venice, so Kumi's own Venice AI is hidden",
            Some("To use Kumi's Venice AI, rename that server in settings.json (modelServers), then sign in with /login".into())
        )
    );
}
