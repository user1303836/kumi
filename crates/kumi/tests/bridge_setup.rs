//! Source bridge setup scenarios, with npm packaging replaced by native receipt-bound artifacts.
use futures::FutureExt;
use kumi::{
    bridge_setup::*,
    input::{ByteListener, TerminalInput},
    tui::tty::TtyOutput,
};
use kumi_runtime::system::{self, Env, SystemProgram};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    fs,
    path::PathBuf,
    rc::Rc,
};
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
#[derive(Debug, Clone)]
struct Call {
    command: String,
    args: Vec<String>,
}
struct World {
    root: tempfile::TempDir,
    bridge: PathBuf,
    scripts: PathBuf,
    state: PathBuf,
    prepared: PathBuf,
    out: Rc<Out>,
    calls: Rc<RefCell<Vec<Call>>>,
    answers: Rc<RefCell<VecDeque<Ran>>>,
}
fn package(root: &std::path::Path, version: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("package.json"), json!({"version":version}).to_string()).unwrap();
    fs::write(root.join(executable_name("ableton-mcp-server")), "nativefixture").unwrap();
}
fn extension(root: &std::path::Path, code: &str) {
    fs::create_dir_all(root.join("live-extension/dist")).unwrap();
    fs::write(root.join("live-extension/manifest.json"), json!({"version":"1.0.0"}).to_string()).unwrap();
    fs::write(root.join("live-extension/dist/extension.js"), code).unwrap();
}
fn answered(value: serde_json::Value) -> Ran {
    Ran { code: 0, stdout: format!("{value}\n"), stderr: "".into() }
}
impl World {
    fn new(installed: Option<&str>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let bridge = root.path().join("bundled");
        package(&bridge, "1.0.34");
        let scripts = root.path().join("Remote Scripts");
        fs::create_dir_all(scripts.join("AbletonMcpBridge")).unwrap();
        let state = root.path().join("state");
        if let Some(version) = installed {
            let package_root = root.path().join("installed");
            package(&package_root, version);
            fs::create_dir_all(&state).unwrap();
            let config = state.join("bridge-config.json");
            fs::write(
                &config,
                json!({"server":{"command":package_root.join(executable_name("ableton-mcp-server")),"args":["--config",config]}})
                    .to_string(),
            )
            .unwrap();
            fs::write(scripts.join("AbletonMcpBridge/bridge-reference.json"), json!({"config":config}).to_string()).unwrap();
        }
        let prepared = root.path().join("prepared");
        package(&prepared.join("package"), "1.0.34");
        extension(&prepared.join("package"), "module.exports = {};\n");
        fs::write(prepared.join("bridge.tar.gz"), b"tarball bytes").unwrap();
        fs::write(
            prepared.join("prepared.json"),
            json!({"artifact":"bridge.tar.gz","sha256":hex::encode(Sha256::digest(b"tarball bytes"))}).to_string(),
        )
        .unwrap();
        Self {
            root,
            bridge,
            scripts,
            state,
            prepared,
            out: Rc::new(Out::default()),
            calls: Rc::new(RefCell::new(vec![])),
            answers: Rc::new(RefCell::new(VecDeque::new())),
        }
    }
    fn io(&self) -> BridgeSetupIo {
        let mut io = BridgeSetupIo::new(
            self.out.clone(),
            Env::from([
                ("KUMI_REMOTE_SCRIPTS_DIR".into(), self.scripts.display().to_string()),
                ("KUMI_LIVE_EXTENSIONS_DIR".into(), self.root.path().join("Ableton/Extensions").display().to_string()),
            ]),
        );
        io.bridge_dir = Some(self.bridge.display().to_string());
        io.home = Some(self.root.path().join("kumi").display().to_string());
        io.prepared = Some(self.prepared.display().to_string());
        io.wait_ms = Some(0);
        io.yes = true;
        io.live_running = Some(Rc::new(|| async { false }.boxed_local()));
        io.remote_script_answers = Some(Rc::new(|_| async { true }.boxed_local()));
        io.sleep = Some(Rc::new(|_| async {}.boxed_local()));
        io.run = Some(Rc::new({
            let calls = self.calls.clone();
            let answers = self.answers.clone();
            let source = self.prepared.clone();
            move |command, args, _cwd| {
                calls.borrow_mut().push(Call { command: command.clone(), args: args.clone() });
                let answers = answers.clone();
                let source = source.clone();
                async move {
                    if command == "python3" || command == "python" {
                        let out = PathBuf::from(&args[args.iter().position(|a| a == "--out").unwrap() + 1]);
                        fs::copy(source.join("prepared.json"), out.join("prepared.json")).unwrap();
                        fs::copy(source.join("bridge.tar.gz"), out.join("bridge.tar.gz")).unwrap();
                        return Ran::default();
                    }
                    if command == system::system_program_default(SystemProgram::Tar) {
                        let folder = &args[args.iter().position(|a| a == "-C").unwrap() + 1];
                        copy_tree(source.join("package").to_str().unwrap(), &format!("{folder}/package")).await.unwrap();
                        return Ran::default();
                    }
                    assert!(command.ends_with(&executable_name("ableton-mcp-server")));
                    assert_eq!(args[0], "lifecycle");
                    answers
                        .borrow_mut()
                        .pop_front()
                        .unwrap_or_else(|| answered(json!({"version":"ableton-mcp-lifecycle/v1","state":"completed"})))
                }
                .boxed_local()
            }
        }));
        io
    }
}
fn flag<'a>(call: &'a Call, name: &str) -> &'a str {
    &call.args[call.args.iter().position(|s| s == name).unwrap() + 1]
}
#[tokio::test(flavor = "current_thread")]
async fn current_bridge_is_left_alone_and_live_open_or_confirmation_declined_changes_nothing() {
    let w = World::new(Some("1.0.34"));
    assert_eq!(setup_bridge(w.io()).await.unwrap(), 0);
    assert!(w.out.0.borrow().contains("bridge 1.0.34 is installed, the same as Kumi's"));
    assert!(w.calls.borrow().is_empty());
    let w = World::new(Some("1.0.33"));
    let mut io = w.io();
    io.live_running = Some(Rc::new(|| async { true }.boxed_local()));
    assert_eq!(setup_bridge(io).await.unwrap(), 1);
    assert!(w.out.0.borrow().contains("Live is open. Save your work, quit Live"));
    assert!(w.calls.borrow().is_empty());
    let mut io = w.io();
    io.yes = false;
    io.confirm = Some(Rc::new(|_| async { false }.boxed_local()));
    assert_eq!(setup_bridge(io).await.unwrap(), 1);
    assert!(w.out.0.borrow().contains("Nothing was changed"));
    assert!(w.calls.borrow().is_empty());
}
#[tokio::test(flavor = "current_thread")]
async fn after_an_update_a_bridge_left_for_later_says_so_and_isnt_a_failure() {
    // Kumi's updater sets KUMI_BRIDGE_AFTER, and so does the migration shim for an older Kumi's.
    let w = World::new(Some("1.0.33"));
    let mut io = w.io();
    io.env.insert("KUMI_BRIDGE_AFTER".into(), "1".into());
    io.yes = false;
    io.confirm = Some(Rc::new(|_| async { false }.boxed_local()));
    assert_eq!(setup_bridge(io).await.unwrap(), 0);
    let said = w.out.0.borrow().clone();
    assert!(said.contains("The bridge wasn't updated yet: quit Live, then run:"), "{said}");
    assert!(!said.contains("Nothing was changed"), "{said}");
    assert!(w.calls.borrow().is_empty());
    let w = World::new(Some("1.0.33"));
    let mut io = w.io();
    io.env.insert("KUMI_BRIDGE_AFTER".into(), "1".into());
    io.live_running = Some(Rc::new(|| async { true }.boxed_local()));
    assert_eq!(setup_bridge(io).await.unwrap(), 0);
    assert!(w.out.0.borrow().contains("The bridge wasn't updated yet: save your work, quit Live, then run:"));
    assert!(w.calls.borrow().is_empty());
    // The shim marks every command it forwards with KUMI_LEGACY_HANDOFF: alone, it's a kumi bridge the
    // producer runs through an older launcher.
    let w = World::new(Some("1.0.33"));
    let mut io = w.io();
    io.env.insert("KUMI_LEGACY_HANDOFF".into(), "1".into());
    io.yes = false;
    io.confirm = Some(Rc::new(|_| async { false }.boxed_local()));
    assert_eq!(setup_bridge(io).await.unwrap(), 1);
    assert!(w.out.0.borrow().contains("Nothing was changed. Quit Live, then run:"));
}
#[tokio::test(flavor = "current_thread")]
async fn developer_bridge_artifact_is_prepared_then_native_lifecycle_plans_before_apply() {
    let w = World::new(None);
    let mut io = w.io();
    io.prepared = Some(w.root.path().join("missing-prepared").display().to_string());
    assert_eq!(setup_bridge(io).await.unwrap(), 0);
    let calls = w.calls.borrow();
    assert!(matches!(calls[0].command.as_str(), "python" | "python3"));
    assert!(calls[0].args.contains(&"--bridge-only".into()));
    assert_eq!(calls[1].command, system::system_program_default(SystemProgram::Tar));
    let plan = &calls[2];
    let apply = &calls[3];
    assert_eq!(plan.args[1], "install");
    assert_eq!(flag(plan, "--remote-scripts-dir"), w.scripts.to_str().unwrap());
    assert_eq!(flag(plan, "--state-dir"), w.root.path().join("kumi").join("bridge").join("state").to_str().unwrap());
    assert_eq!(flag(plan, "--artifact-sha256").len(), 64);
    assert!(!plan.args.contains(&"--apply".into()));
    assert!(apply.args.contains(&"--apply".into()));
    assert!(apply.args.contains(&"--confirm-live-stopped".into()));
    assert!(!apply.args.contains(&"--allow-dirty-private-build".into()));
    assert!(w.out.0.borrow().contains("choose AbletonMcpBridge as a Control Surface"));
}
#[tokio::test(flavor = "current_thread")]
async fn prepared_bridge_is_copied_without_build_tools_and_checksum_mismatch_refuses() {
    let w = World::new(None);
    assert_eq!(setup_bridge(w.io()).await.unwrap(), 0);
    let before = w.calls.borrow().len();
    assert_eq!(before, 2);
    let call = w.calls.borrow()[0].clone();
    assert_eq!(fs::read(flag(&call, "--artifact")).unwrap(), b"tarball bytes");
    assert!(PathBuf::from(flag(&call, "--package-root")).join(executable_name("ableton-mcp-server")).exists());
    assert!(w
        .out
        .0
        .borrow()
        .contains("Copying the bridge…\nInstalling Live's Remote Script and the bridge…\nDone: the Ableton bridge 1.0.34 is installed"));
    fs::write(w.prepared.join("prepared.json"), json!({"artifact":"bridge.tar.gz","sha256":"0".repeat(64)}).to_string()).unwrap();
    assert_eq!(setup_bridge(w.io()).await.unwrap(), 1);
    assert!(w.out.0.borrow().contains("Kumi's copy of the bridge is damaged"));
    assert_eq!(w.calls.borrow().len(), before);
}
#[tokio::test(flavor = "current_thread")]
async fn upgrade_keeps_state_and_explains_plan_or_apply_refusal() {
    let w = World::new(Some("1.0.33"));
    w.answers.borrow_mut().push_back(Ran {
        code: 1,
        stdout: "".into(),
        stderr: format!(
            "{}\n",
            json!({"version":"ableton-mcp-lifecycle-error/v1","reason":"private build is dirty; pass --allow-dirty-private-build"})
        ),
    });
    assert_eq!(setup_bridge(w.io()).await.unwrap(), 1);
    let call = w.calls.borrow()[0].clone();
    assert_eq!(call.args[1], "upgrade");
    assert_eq!(flag(&call, "--state-dir"), w.state.to_str().unwrap());
    assert!(w.out.0.borrow().contains("installer refused: private build is dirty"));
    assert!(w.out.0.borrow().contains("add --allow-dirty"));
    w.answers
        .borrow_mut()
        .extend([answered(json!({"state":"planned"})), Ran { code: 1, stdout: "".into(), stderr: "apply failed\n".into() }]);
    assert_eq!(setup_bridge(w.io()).await.unwrap(), 1);
    assert!(w.out.0.borrow().contains("stopped, and put back what was there: apply failed"));
}
#[tokio::test(flavor = "current_thread")]
async fn live_opened_again_before_the_switch_changes_nothing() {
    let w = World::new(Some("1.0.33"));
    let mut io = w.io();
    // Closed at the first look, open again by the switch.
    let looks = Rc::new(Cell::new(0));
    io.live_running = Some(Rc::new({
        let looks = looks.clone();
        move || {
            looks.set(looks.get() + 1);
            let open = looks.get() > 1;
            async move { open }.boxed_local()
        }
    }));
    assert_eq!(setup_bridge(io).await.unwrap(), 1);
    assert_eq!(looks.get(), 2);
    assert!(w.out.0.borrow().contains("Live is open again, so nothing was changed"), "{}", w.out.0.borrow());
    let calls = w.calls.borrow();
    assert!(calls.iter().all(|call| !call.args.contains(&"--apply".into())), "planned, never applied: {calls:?}");
    let left: Vec<_> =
        fs::read_dir(w.root.path().join("kumi/bridge")).map(|d| d.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
    assert!(left.is_empty(), "the unpacked version is gone: {left:?}");
}
#[tokio::test(flavor = "current_thread")]
async fn the_in_app_install_keeps_quiet_and_says_what_went_wrong_in_a_sentence() {
    // Done: the bundled bridge's version, nothing printed, nothing asked, no wait for Live.
    let w = World::new(None);
    let mut io = w.io();
    io.wait_ms = None;
    io.yes = false;
    assert_eq!(install_quietly(io).await, Ok("1.0.34".into()));
    assert!(w.out.0.borrow().is_empty(), "{}", w.out.0.borrow());
    // Live open: the setup's to fix, said as such.
    let w = World::new(Some("1.0.33"));
    let mut io = w.io();
    io.live_running = Some(Rc::new(|| async { true }.boxed_local()));
    assert_eq!(install_quietly(io).await, Err("Live is open again: quit it, then Try again.".into()));
    // Live opening again just before the switch says the same.
    let w = World::new(Some("1.0.33"));
    let mut io = w.io();
    let looks = Rc::new(Cell::new(0));
    io.live_running = Some(Rc::new({
        let looks = looks.clone();
        move || {
            looks.set(looks.get() + 1);
            let open = looks.get() > 1;
            async move { open }.boxed_local()
        }
    }));
    assert_eq!(install_quietly(io).await, Err("Live is open again: quit it, then Try again.".into()));
    // Anything else: the last thing the installer said.
    let w = World::new(Some("1.0.33"));
    w.answers
        .borrow_mut()
        .extend([answered(json!({"state":"planned"})), Ran { code: 1, stdout: "".into(), stderr: "apply failed\n".into() }]);
    assert_eq!(install_quietly(w.io()).await, Err("The bridge's installer stopped, and put back what was there: apply failed".into()));
}
#[tokio::test(flavor = "current_thread")]
async fn activation_waits_until_remote_script_answers_and_records_connection() {
    let w = World::new(Some("1.0.33"));
    w.answers.borrow_mut().extend([
        answered(json!({"state":"planned"})),
        answered(json!({"state":"installed-restart-required"})),
        answered(json!({"state":"activation-required","verification":{"liveConnected":false}})),
        answered(json!({"state":"completed","verification":{"liveConnected":true}})),
    ]);
    let mut io = w.io();
    io.wait_ms = Some(60000);
    io.allow_dirty = true;
    assert_eq!(setup_bridge(io).await.unwrap(), 0);
    let calls = w.calls.borrow();
    assert_eq!(calls.iter().filter(|c| c.args[1] == "activate").count(), 2);
    assert!(calls.iter().all(|c| c.args.contains(&"--allow-dirty-private-build".into())));
    assert!(w.out.0.borrow().contains("Live is connected through the new bridge"));
    drop(calls);
    let w = World::new(Some("1.0.33"));
    w.answers.borrow_mut().extend([
        answered(json!({"state":"planned"})),
        answered(json!({"state":"installed-restart-required"})),
        answered(json!({"state":"completed","verification":{"liveConnected":true}})),
    ]);
    let asked = Rc::new(Cell::new(0));
    let slept = Rc::new(Cell::new(0));
    let mut io = w.io();
    io.wait_ms = Some(60000);
    io.remote_script_answers = Some(Rc::new({
        let asked = asked.clone();
        move |_| {
            asked.set(asked.get() + 1);
            let yes = asked.get() > 3;
            async move { yes }.boxed_local()
        }
    }));
    io.sleep = Some(Rc::new({
        let slept = slept.clone();
        move |_| {
            slept.set(slept.get() + 1);
            async {}.boxed_local()
        }
    }));
    assert_eq!(setup_bridge(io).await.unwrap(), 0);
    assert_eq!(asked.get(), 4);
    assert_eq!(slept.get(), 3);
    assert_eq!(w.calls.borrow().iter().filter(|c| c.args[1] == "activate").count(), 1);
}
#[derive(Default)]
struct Input {
    listener: RefCell<Option<ByteListener>>,
    raw: RefCell<Vec<bool>>,
}
impl TerminalInput for Input {
    fn is_tty(&self) -> bool {
        true
    }
    fn set_raw_mode(&self, v: bool) -> std::io::Result<()> {
        self.raw.borrow_mut().push(v);
        Ok(())
    }
    fn resume(&self, next: ByteListener) {
        *self.listener.borrow_mut() = Some(next)
    }
    fn pause(&self) {
        self.listener.borrow_mut().take();
    }
}
#[tokio::test(flavor = "current_thread")]
async fn enter_control_c_or_escape_stops_waiting_and_restores_raw_mode() {
    for key in [b'\r', 3, 27] {
        let w = World::new(Some("1.0.33"));
        let input = Rc::new(Input::default());
        let mut io = w.io();
        io.input = Some(input.clone());
        io.wait_ms = Some(60000);
        io.remote_script_answers = Some(Rc::new({
            let input = input.clone();
            move |_| {
                let input = input.clone();
                async move {
                    let next = input.listener.borrow().clone();
                    next.unwrap()(&[key]);
                    false
                }
                .boxed_local()
            }
        }));
        io.sleep = Some(Rc::new(|_| async { std::future::pending::<()>().await }.boxed_local()));
        assert_eq!(setup_bridge(io).await.unwrap(), 0);
        assert_eq!(*input.raw.borrow(), [true, false]);
        assert!(w.out.0.borrow().contains("Stopped waiting. Kumi connects on its own"));
        assert!(!w.calls.borrow().iter().any(|c| c.args[1] == "activate"));
    }
}
#[tokio::test(flavor = "current_thread")]
async fn user_library_is_required_but_remote_scripts_folder_can_be_created() {
    let w = World::new(None);
    let library = w.root.path().join("Library Moved/User Library");
    fs::create_dir_all(&library).unwrap();
    let mut io = w.io();
    io.env.insert("KUMI_REMOTE_SCRIPTS_DIR".into(), library.join("Remote Scripts").display().to_string());
    assert_eq!(setup_bridge(io).await.unwrap(), 0);
    assert!(library.join("Remote Scripts").exists());
    let w = World::new(None);
    let mut io = w.io();
    io.env.insert("KUMI_REMOTE_SCRIPTS_DIR".into(), w.root.path().join("missing/User Library/Remote Scripts").display().to_string());
    assert_eq!(setup_bridge(io).await.unwrap(), 1);
    assert!(w.out.0.borrow().contains("Kumi couldn't find Live's User Library"));
    assert!(w.calls.borrow().is_empty());
}
#[tokio::test(flavor = "current_thread")]
async fn extension_installs_updates_or_stays_unchanged_and_missing_live_folder_is_nonfatal() {
    let w = World::new(None);
    fs::create_dir_all(w.root.path().join("Ableton")).unwrap();
    assert_eq!(setup_bridge(w.io()).await.unwrap(), 0);
    assert_eq!(fs::read_to_string(w.root.path().join("Ableton/Extensions/kumi.kumi/dist/extension.js")).unwrap(), "module.exports = {};\n");
    assert!(w.out.0.borrow().contains("Added Kumi's extension to Live"));
    let current = World::new(Some("1.0.34"));
    fs::create_dir_all(current.root.path().join("Ableton")).unwrap();
    let carried = current.root.path().join("installed");
    extension(&carried, "installed();\n");
    assert_eq!(setup_bridge(current.io()).await.unwrap(), 0);
    extension(&carried, "newer();\n");
    assert_eq!(setup_bridge(current.io()).await.unwrap(), 0);
    assert!(current.out.0.borrow().contains("Updated Kumi's extension in Live."));
    let before = current.out.0.borrow().len();
    assert_eq!(setup_bridge(current.io()).await.unwrap(), 0);
    assert!(!current.out.0.borrow()[before..].contains("extension"));
    let missing = World::new(None);
    assert_eq!(setup_bridge(missing.io()).await.unwrap(), 0);
    assert!(!missing.root.path().join("Ableton/Extensions").exists());
    assert!(missing.out.0.borrow().contains("Kumi couldn't add its extension to Live (Live's folder isn't there"));
}
#[tokio::test(flavor = "current_thread")]
async fn windows_live_detection_uses_powershell_and_falls_back_only_after_failure() {
    for (powershell, tasklist, want, calls_want) in [
        (Ran { stdout: "1138176\r\n".into(), ..Default::default() }, Ran::default(), true, 1),
        (Ran::default(), Ran { stdout: "Ableton Live 12 Suite.exe".into(), ..Default::default() }, false, 1),
        (
            Ran { code: 1, ..Default::default() },
            Ran { stdout: "Ableton Live 12 Suite.exe 4242 Console".into(), ..Default::default() },
            true,
            2,
        ),
    ] {
        let asked = Rc::new(RefCell::new(vec![]));
        let run: Run = Rc::new({
            let asked = asked.clone();
            move |command, _, _| {
                asked.borrow_mut().push(command.clone());
                let result = if command.to_lowercase().ends_with("powershell.exe") { powershell.clone() } else { tasklist.clone() };
                async move { result }.boxed_local()
            }
        });
        assert_eq!(is_live_running_on(run, "win32", &Env::new()).await, want);
        assert_eq!(asked.borrow().len(), calls_want);
    }
}
#[tokio::test]
async fn remote_script_probe_uses_only_loopback_and_real_runner_captures_exit_and_output() {
    use tokio::net::TcpListener;
    let folder = tempfile::tempdir().unwrap();
    let config = folder.path().join("config.json");
    assert!(remote_script_answers(config.to_str().unwrap()).await);
    fs::write(&config, json!({"bridge":{"host":"remote.invalid","port":1}}).to_string()).unwrap();
    assert!(remote_script_answers(config.to_str().unwrap()).await);
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    fs::write(&config, json!({"bridge":{"host":"127.0.0.1","port":port}}).to_string()).unwrap();
    assert!(remote_script_answers(config.to_str().unwrap()).await);
    drop(socket);
    assert!(!remote_script_answers(config.to_str().unwrap()).await);
    #[cfg(unix)]
    {
        let ran = run_program("/bin/sh", &["-c".into(), "printf output; printf problem >&2; exit 7".into()], None).await;
        assert_eq!(ran, Ran { code: 7, stdout: "output".into(), stderr: "problem".into() });
    }
    assert_eq!(run_program("/definitely/missing/kumi-fixture", &[], None).await.code, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn same_version_legacy_bridge_uses_native_lifecycle_upgrade() {
    let w = World::new(Some("1.0.34"));
    let config = w.state.join("bridge-config.json");
    fs::write(
        &config,
        json!({"server":{"command":"node","args":[w.root.path().join("installed/dist/src/index.js"),"--config",config]}}).to_string(),
    )
    .unwrap();
    assert_eq!(setup_bridge(w.io()).await.unwrap(), 0);
    let calls = w.calls.borrow();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].args[1], "upgrade");
    assert!(!calls[0].args.contains(&"--apply".into()));
    assert!(calls[1].args.contains(&"--apply".into()));
    assert!(!w.out.0.borrow().contains("same as Kumi's"));
}

#[tokio::test(flavor = "current_thread")]
async fn discoverable_owner_receipt_preserves_custom_config_secret_and_state_paths() {
    let w = World::new(Some("1.0.33"));
    let config = w.root.path().join("custom/connection.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::rename(w.state.join("bridge-config.json"), &config).unwrap();
    fs::write(w.scripts.join("AbletonMcpBridge/bridge-reference.json"), json!({"config":config}).to_string()).unwrap();
    let state = w.root.path().join("kumi/bridge/state");
    fs::create_dir_all(&state).unwrap();
    let secret = w.root.path().join("custom/token");
    let receipt = state.join("install-receipt.json");
    fs::write(&receipt, json!({"version":1,"stateDirectory":state,"configPath":config,"secretPath":secret,"remoteScriptsDirectory":w.scripts,"packageRoot":w.root.path().join("installed")}).to_string()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert_eq!(setup_bridge(w.io()).await.unwrap(), 0);
    for call in w.calls.borrow().iter() {
        assert_eq!(flag(call, "--state-dir"), state.to_str().unwrap());
        assert_eq!(flag(call, "--config"), config.to_str().unwrap());
        assert_eq!(flag(call, "--secret"), secret.to_str().unwrap());
        assert_eq!(flag(call, "--remote-scripts-dir"), w.scripts.to_str().unwrap());
    }
}
/// After an app rollback: a newer Kumi's bridge (1.0.35) in Live and its extension, with a receipt keeping `kept` (a
/// bridge's version) to go back to, installed from this Kumi's bundled artifact or, when `ours` is false, another.
fn after_an_app_rollback(kept: Option<&str>, ours: bool) -> World {
    let w = World::new(Some("1.0.35"));
    let native = json!({"schema":"ableton-mcp-native-release/v1"}).to_string();
    let installed = w.root.path().join("installed");
    fs::write(installed.join("release-manifest.json"), &native).unwrap();
    extension(&installed, "// the newer bridge's\n");
    let mut previous = serde_json::Value::Null;
    if let Some(version) = kept {
        let root = w.root.path().join("kept");
        package(&root, version);
        fs::write(root.join("release-manifest.json"), &native).unwrap();
        extension(&root, "// this Kumi's bridge's\n");
        let artifact = if ours { hex::encode(Sha256::digest(b"tarball bytes")) } else { "0".repeat(64) };
        previous = json!({"packageRoot":root,"artifactSha256":artifact});
    }
    let live = w.root.path().join("Ableton/Extensions/kumi.kumi");
    fs::create_dir_all(live.join("dist")).unwrap();
    fs::copy(installed.join("live-extension/manifest.json"), live.join("manifest.json")).unwrap();
    fs::copy(installed.join("live-extension/dist/extension.js"), live.join("dist/extension.js")).unwrap();
    let receipt = w.state.join("install-receipt.json");
    fs::write(&receipt, json!({"version":1,"stateDirectory":w.state,"configPath":w.state.join("bridge-config.json"),"secretPath":w.state.join("bridge.secret"),"remoteScriptsDirectory":w.scripts,"packageRoot":installed,"previous":previous}).to_string()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600)).unwrap();
    }
    w
}
fn live_extension(w: &World) -> String {
    fs::read_to_string(w.root.path().join("Ableton/Extensions/kumi.kumi/dist/extension.js")).unwrap()
}
#[tokio::test(flavor = "current_thread")]
async fn after_an_app_rollback_the_kept_bridge_of_this_kumi_goes_back_with_its_extension() {
    let w = after_an_app_rollback(Some("1.0.34"), true);
    let installed = w.root.path().join("installed");
    let kept = w.root.path().join("kept");
    let mut io = w.io();
    io.wait_ms = Some(2000);
    w.answers.borrow_mut().extend([answered(json!({"state":"completed"})), answered(json!({"state":"activated"}))]);
    assert_eq!(setup_bridge(io).await.unwrap(), 0);
    let said = w.out.0.borrow().clone();
    assert!(said.contains("The bridge in Live is 1.0.35, from a newer Kumi. It works with this one; this Kumi's own, 1.0.34"), "{said}");
    assert!(said.contains("Done: the Ableton bridge 1.0.34 is back"), "{said}");
    assert!(said.contains("Kumi's extension in Live went back with it."), "{said}");
    assert!(said.contains("Live is connected through this Kumi's bridge again"), "{said}");
    let calls = w.calls.borrow();
    assert_eq!(calls.len(), 2);
    // The installed bridge's own rollback, which checks the kept generation against its own registry.
    assert_eq!(calls[0].command, installed.join(executable_name("ableton-mcp-server")).to_str().unwrap());
    assert_eq!(&calls[0].args[..2], ["lifecycle", "rollback"]);
    assert_eq!(flag(&calls[0], "--package-root"), installed.to_str().unwrap());
    assert_eq!(flag(&calls[0], "--state-dir"), w.state.to_str().unwrap());
    assert_eq!(flag(&calls[0], "--remote-scripts-dir"), w.scripts.to_str().unwrap());
    assert!(calls[0].args.contains(&"--confirm-live-stopped".into()));
    // Live's connection is recorded by the bridge that went back.
    assert_eq!(calls[1].command, kept.join(executable_name("ableton-mcp-server")).to_str().unwrap());
    assert_eq!(&calls[1].args[..2], ["lifecycle", "activate"]);
    assert_eq!(flag(&calls[1], "--package-root"), kept.to_str().unwrap());
    assert_eq!(live_extension(&w), "// this Kumi's bridge's\n");
}
#[tokio::test(flavor = "current_thread")]
async fn after_an_app_rollback_live_open_a_no_or_a_refused_rollback_changes_nothing() {
    let w = after_an_app_rollback(Some("1.0.34"), true);
    let mut io = w.io();
    io.live_running = Some(Rc::new(|| async { true }.boxed_local()));
    assert_eq!(setup_bridge(io).await.unwrap(), 1);
    assert!(w.out.0.borrow().contains("Live is open. To put back 1.0.34, save your work, quit Live, then run this again:"));
    let mut io = w.io();
    io.yes = false;
    io.confirm = Some(Rc::new(|_| async { false }.boxed_local()));
    assert_eq!(setup_bridge(io).await.unwrap(), 1);
    assert!(w.out.0.borrow().contains("Nothing was changed. To put back 1.0.34, quit Live, then run:"));
    // Closed at the first look, open again once the question is answered.
    let mut io = w.io();
    io.yes = false;
    io.confirm = Some(Rc::new(|_| async { true }.boxed_local()));
    let looks = Rc::new(Cell::new(0));
    io.live_running = Some(Rc::new(move || {
        looks.set(looks.get() + 1);
        let open = looks.get() > 1;
        async move { open }.boxed_local()
    }));
    assert_eq!(setup_bridge(io).await.unwrap(), 1);
    assert!(w.out.0.borrow().contains("Live is open again, so nothing was changed."));
    assert!(w.calls.borrow().is_empty());
    w.answers.borrow_mut().push_back(Ran {
        code: 1,
        stdout: "".into(),
        stderr: format!(
            "{}\n",
            json!({"version":"ableton-mcp-lifecycle-error/v1","reason":"no verified previous generation is available"})
        ),
    });
    assert_eq!(setup_bridge(w.io()).await.unwrap(), 1);
    assert!(w.out.0.borrow().contains("The bridge's rollback stopped, and put back what was there: no verified previous generation"));
    assert_eq!(w.calls.borrow().len(), 1);
    assert_eq!(live_extension(&w), "// the newer bridge's\n");
}
#[tokio::test(flavor = "current_thread")]
async fn after_an_app_rollback_a_newer_bridge_without_this_kumis_kept_stays_and_works() {
    // None kept, another version kept, or this version from another artifact (a checkout's, say).
    for (kept, ours) in [(None, true), (Some("1.0.33"), true), (Some("1.0.34"), false)] {
        let w = after_an_app_rollback(kept, ours);
        assert_eq!(setup_bridge(w.io()).await.unwrap(), 0);
        let said = w.out.0.borrow().clone();
        assert!(said.contains("The bridge in Live is 1.0.35, from a newer Kumi. It works with this one, so it stays."), "{said}");
        assert!(!said.contains("Updating it takes a minute"), "{said}");
        assert!(w.calls.borrow().is_empty());
        assert_eq!(live_extension(&w), "// the newer bridge's\n");
    }
}
