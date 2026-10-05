//! `kumi doctor`, and voice readiness checks.
use futures::FutureExt;
use kumi::{doctor::*, live_extension::install_extension, tui::tty::TtyOutput};
use kumi_common::time::now_ms;
use kumi_runtime::{
    providers::{local::local_servers, models::ModelInfo},
    system::Env,
    voice::VoiceReadiness,
    KUMI, KUMI_REPAIR,
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
        ]);
        Self { root, env, package, config }
    }
    fn io(&self) -> DoctorIo {
        let mut io = DoctorIo::new(Rc::new(Out::default()), self.env.clone());
        io.node_version = Some("v24.21.0".into());
        io.terminal = Some(TerminalInfo { is_tty: true, columns: Some(120), rows: Some(36) });
        io.probe_live = Some(Rc::new(|_| {
            async {
                Ok(LiveProbe {
                    started: true,
                    connected: Some(true),
                    set: Some("Night Drive".into()),
                    real_live: Some(true),
                    ..Default::default()
                })
            }
            .boxed_local()
        }));
        io.node_version_of = Some(Rc::new(|_| async { Some("v24.1.0".into()) }.boxed_local()));
        io.video_programs = Some(Rc::new(|| {
            async { Ok(VideoPrograms { ffmpeg: Some("/usr/bin/ffmpeg".into()), whisper: Some("/usr/bin/whisper-cli".into()) }) }
                .boxed_local()
        }));
        io.hands = Some(Rc::new(|| async { Ok(None) }.boxed_local()));
        io.voice = Some(Rc::new(|| async { Ok(ready()) }.boxed_local()));
        io.model_servers = Some(Rc::new(|| async { Ok(vec![]) }.boxed_local()));
        io
    }
}
fn find<'a>(checks: &'a [Check], text: &str) -> &'a Check {
    checks.iter().find(|c| c.text.contains(text)).unwrap_or_else(|| panic!("No {text:?} in {checks:?}"))
}
fn probe(io: &mut DoctorIo, live: LiveProbe) {
    io.probe_live = Some(Rc::new(move |_| {
        let live = live.clone();
        async move { Ok(live) }.boxed_local()
    }));
}
#[tokio::test(flavor = "current_thread")]
async fn doctor_says_whats_fine_and_exact_fixes_without_secrets() {
    let s = Setup::new();
    let out = Rc::new(Out::default());
    let mut io = s.io();
    io.out = out.clone();
    io.node_version = Some("v20.11.0".into());
    io.bundled_bridge_version = Some("1.0.10".into());
    assert_eq!(run_doctor(io.clone()).await.unwrap(), 1);
    let printed = out.0.borrow().clone();
    for text in [
        "Node.js 20.11.0 isn't supported (Kumi needs 22 or newer)",
        "Install Node 24 LTS",
        "openai API key from OPENAI_API_KEY · model openai/gpt-fixture",
        "The installed bridge (1.0.9) is older than this Kumi's (1.0.10)",
        "Other MCP apps would start the bridge with a Node from a temporary folder",
        "Live connected · Night Drive",
        "2 things to fix",
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
    probe(&mut io, LiveProbe { started: true, connected: Some(false), ..Default::default() });
    assert_eq!(find(&doctor_checks(&io).await.unwrap(), "Live isn't connected").status, CheckStatus::Fix);
    probe(&mut io, LiveProbe::default());
    assert!(find(&doctor_checks(&io).await.unwrap(), "didn't start").next.as_ref().unwrap().contains(*KUMI_REPAIR));
    io.bundled_bridge_version = Some("1.0.9".into());
    let checks = doctor_checks(&io).await.unwrap();
    assert!(find(&checks, "couldn't reach Live").next.as_ref().unwrap().contains("answer it first"));
    assert!(!checks.iter().any(|c| c.text.contains("didn't start")));
    io.terminal = Some(TerminalInfo { is_tty: true, columns: Some(50), rows: Some(12) });
    assert!(find(&doctor_checks(&io).await.unwrap(), "Terminal").text.contains("small"));
    io.env.insert("KUMI_REMOTE_SCRIPTS_DIR".into(), s.root.path().join("nowhere").display().to_string());
    assert!(find(&doctor_checks(&io).await.unwrap(), "bridge isn't installed")
        .next
        .as_ref()
        .unwrap()
        .contains(&format!("{} bridge", *KUMI)));
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
    probe(&mut io, LiveProbe { started: true, connected: Some(false), ..Default::default() });
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
    probe(&mut io, LiveProbe { started: true, live_version: Some("12.3.2".into()), ..Default::default() });
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
    io.hands = Some(Rc::new({
        let off = off.clone();
        move || {
            let off = off.clone();
            async move { Ok(Some(off)) }.boxed_local()
        }
    }));
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
    fs::write(binary.parent().unwrap().join("package.json"), json!({"version":kumi_runtime::KUMI_VERSION,"bridge":"1.0.11"}).to_string())
        .unwrap();
    fs::write(&s.config, json!({"version":2,"server":{"command":binary,"args":["--config",s.config]}}).to_string()).unwrap();
    let server = read_bridge_server(s.config.to_str().unwrap()).unwrap();
    assert!(server.native());
    assert_eq!(server.entry, Some(binary.display().to_string()));
    assert_eq!(server.version, Some("1.0.11".into()));
    assert_eq!(server.package_root(), Some(binary.parent().unwrap().display().to_string()));
    let mut io = s.io();
    io.node_version = None;
    assert!(doctor_checks(&io).await.unwrap()[0].text.ends_with("(native executable)"));
}
