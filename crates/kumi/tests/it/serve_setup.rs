//! `kumi serve`'s `setup_daw` (C8): Kumi's part in a DAW planned from files alone and put in exactly as planned. Live's
//! Remote Script goes in through the bridge's own lifecycle (the code `ableton-mcp-server lifecycle` runs) or a stand-in
//! that writes what it leaves, Kumi Ears for real; Renoise's tool through the shared lifecycle itself. Everything is in
//! temporary folders: never the real ~/.kumi or a DAW's.

use futures::FutureExt;
use kumi::serve::ctx::{Call, Ctx, Host as HostCall, OpError, Progress};
use kumi::serve::lease::Lease;
use kumi::serve::{registry, Args};
use kumi_common::abort::Signal;
use kumi_daw::process::{executable_name, Ran, Run};
use kumi_daw_ableton::app::answers::{LiveRunning, PreparedBridge};
use kumi_daw_own::provider::CarriedBy;
use kumi_types::{daw::DawId, system::Env};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

/// Every file under a folder and its bytes (a link as the path it names): what a step wrote is what differs. The
/// runtimes' own state folders (`plugin/state…`, where their snapshots are written as they go) are left out.
type Tree = BTreeMap<PathBuf, Vec<u8>>;

fn tree(root: &Path) -> Tree {
    let mut files = Tree::new();
    let mut folders = vec![root.to_path_buf()];
    let runtimes = root.join("plugin");
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap().flatten() {
            let (path, kind) = (entry.path(), entry.file_type().unwrap());
            if kind.is_dir() && folder == runtimes && entry.file_name().to_string_lossy().starts_with("state") {
                continue;
            } else if kind.is_dir() {
                folders.push(path);
            } else if kind.is_symlink() {
                files.insert(path.clone(), std::fs::read_link(&path).unwrap().display().to_string().into_bytes());
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

/// The files added, changed or gone between two looks.
fn differ(before: &Tree, after: &Tree) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = before.keys().chain(after.keys()).filter(|path| before.get(*path) != after.get(*path)).cloned().collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Each file a step wrote lies in one of the paths its plan named (or is the lease's own file).
fn within(written: &[PathBuf], plan: &Value, lease: &Path) {
    let named: Vec<PathBuf> = plan["paths"].as_array().unwrap().iter().map(|path| PathBuf::from(path.as_str().unwrap())).collect();
    for path in written {
        assert!(path == lease || named.iter().any(|named| path.starts_with(named)), "{} isn't in the plan's {named:?}", path.display());
    }
}

fn sha256(path: &Path) -> String {
    Sha256::digest(std::fs::read(path).unwrap()).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn bundled() -> String {
    kumi_daw_ableton::app::bridge_setup::bundled_bridge_version().unwrap()
}

/// `relative`'s folders (written with `/`) under `base`, each its own component: the path spelled as the system spells
/// it (on Windows, with `\` throughout), as the paths a plan names are.
fn at(base: &Path, relative: &str) -> PathBuf {
    relative.split('/').fold(base.to_path_buf(), |path, part| path.join(part))
}

/// A runtime's bundle of Live's bridge, as a release lays it out: `prepared.json` and the tarball it names, which holds
/// `package/` (the server and its package.json).
fn bundle(root: &Path) -> PathBuf {
    let prepared = at(root, "runtime/bridge");
    let package = prepared.join("package");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join(executable_name("ableton-mcp-server")), b"the bridge\n").unwrap();
    std::fs::write(package.join("package.json"), json!({"version": bundled()}).to_string()).unwrap();
    let packed = std::process::Command::new("tar").args(["-czf", "bridge.tar.gz", "package"]).current_dir(&prepared).output().unwrap();
    assert!(packed.status.success(), "{packed:?}");
    std::fs::remove_dir_all(&package).unwrap();
    let sha = sha256(&prepared.join("bridge.tar.gz"));
    std::fs::write(prepared.join("prepared.json"), json!({"artifact": "bridge.tar.gz", "sha256": sha}).to_string()).unwrap();
    prepared
}

/// A tar archive of `files` by their paths in it (each a regular file, owner-only), as the bridge's release fixture
/// writes one.
fn tar(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, content) in files {
        let mut header = [0u8; 512];
        let mut field = |offset: usize, size: usize, value: &[u8]| {
            header[offset..offset + value.len().min(size)].copy_from_slice(&value[..value.len().min(size)]);
        };
        field(0, 100, name.as_bytes());
        field(100, 8, b"0000600\0");
        field(108, 8, b"0000000\0");
        field(116, 8, b"0000000\0");
        field(124, 12, format!("{:011o}\0", content.len()).as_bytes());
        field(136, 12, b"00000000000\0");
        field(257, 6, b"ustar\0");
        field(263, 2, b"00");
        header[148..156].fill(b' ');
        header[156] = b'0';
        let sum = header.iter().map(|byte| *byte as u64).sum::<u64>();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        tar.extend_from_slice(&header);
        tar.extend_from_slice(content);
        tar.resize(tar.len() + content.len().div_ceil(512) * 512 - content.len(), 0);
    }
    tar.resize(tar.len() + 1024, 0);
    tar
}

/// A runtime's bundle of Live's bridge as a release makes it: a strict native release package (its manifest binding
/// every file, the Remote Script among them) at this Kumi's bundled version, in the tarball `prepared.json` names. The
/// bridge's own lifecycle installs it as it would a release's.
fn release(root: &Path) -> PathBuf {
    use std::io::Write;
    let prepared = at(root, "runtime/bridge");
    let (binary, worker) = (executable_name("ableton-mcp-server"), executable_name("ableton-mcp-analysis-worker"));
    let target = if cfg!(windows) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(target_os = "macos") {
        "aarch64-apple-darwin"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let version = bundled();
    let metadata = json!({"name": "@ableton-mcp/mcp-server", "version": version, "private": true, "license": "MIT", "type": "module",
                          "runtime": "rust-native", "target": target, "bin": {"ableton-mcp-server": binary, "ableton-mcp-analysis-worker": worker}});
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    files.insert("package.json".into(), metadata.to_string().into_bytes());
    files.insert("LICENSE.md".into(), include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/LICENSE.md")).to_vec());
    files.insert(binary.clone(), b"the bridge\n".to_vec());
    files.insert(worker.clone(), b"the analysis worker\n".to_vec());
    files.insert("remote-script/AbletonMcpBridge/__init__.py".into(), b"def create_instance(c_instance):\n    return None\n".to_vec());
    files.insert("remote-script/AbletonMcpBridge/ableton_mcp_remote_script.py".into(), b"class AbletonMcpBridge:\n    pass\n".to_vec());
    for n in 0..9 {
        files.insert(format!("release-docs/doc-{n}.md"), format!("# {n}\n").into_bytes());
    }
    let role = |name: &str| match name {
        "LICENSE.md" => "license",
        "package.json" => "package-metadata",
        _ if name == binary || name == worker => "native-runtime",
        _ if name.starts_with("release-docs/") => "documentation",
        _ => "ableton-remote-script",
    };
    let hash = |bytes: &[u8]| Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let manifest = json!({
        "schema": "ableton-mcp-native-release/v1",
        "package": {"name": "@ableton-mcp/mcp-server", "version": version, "license": "MIT", "private": true},
        "source": {"commit": "0123456789abcdef0123456789abcdef01234567", "dirty": false},
        "build": {"runtime": "rust-native", "target": target, "recipe": "test fixture",
                  "builder": {"rustc": "fixture", "cargo": "fixture", "platform": "fixture", "architecture": "fixture",
                              "runnerImage": "fixture", "runnerImageVersion": "fixture",
                              "cargoLockSha256": "a".repeat(64), "workflowSha256": "b".repeat(64)}},
        "protocol": {"registryHash": ableton_mcp_server::lifecycle::delivery::registry_digest()},
        "distribution": {"channel": "local-native-tarball", "published": false, "signed": false, "notarized": false,
                         "integrityIsIdentityProof": false},
        "algorithm": "sha256",
        "files": files.iter().map(|(name, bytes)| (name.clone(), json!(hash(bytes)))).collect::<serde_json::Map<_, _>>(),
        "roles": files.keys().map(|name| (name.clone(), json!(role(name)))).collect::<serde_json::Map<_, _>>(),
    });
    files.insert("release-manifest.json".into(), manifest.to_string().into_bytes());
    let packed: BTreeMap<String, Vec<u8>> = files.into_iter().map(|(name, bytes)| (format!("package/{name}"), bytes)).collect();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&tar(&packed)).unwrap();
    std::fs::create_dir_all(&prepared).unwrap();
    let artifact = prepared.join("bridge.tgz");
    std::fs::write(&artifact, gzip.finish().unwrap()).unwrap();
    std::fs::write(prepared.join("prepared.json"), json!({"artifact": "bridge.tgz", "sha256": sha256(&artifact)}).to_string()).unwrap();
    prepared
}

/// The bridge's own lifecycle, run in process as `ableton-mcp-server lifecycle …` runs it (the binary's own entry), on
/// two loopback ports free now, so that no Live running here matters to the test.
fn real_lifecycle(calls: Rc<RefCell<Vec<Vec<String>>>>) -> Run {
    let free = || std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let port = free();
    let realtime = std::iter::repeat_with(free).find(|other| *other != port).unwrap();
    Rc::new(move |program: String, args: Vec<String>, _: Option<String>| {
        calls.borrow_mut().push(std::iter::once(program).chain(args.iter().cloned()).collect());
        let mut args = args;
        args.extend(["--port".into(), port.to_string(), "--realtime-port".into(), realtime.to_string()]);
        async move {
            let said = ableton_mcp_server::cli::auxiliary_command(&args).await.expect("a lifecycle command");
            Ran { code: said.code, stdout: said.stdout, stderr: said.stderr }
        }
        .boxed_local()
    })
}

/// The bridge's lifecycle, standing in for the server a real install runs: each call recorded, and an applied install
/// leaves what the real one does in the folders it's given (the Remote Script with its reference to the configuration,
/// the configuration starting the server in the package given).
fn lifecycle(calls: Rc<RefCell<Vec<Vec<String>>>>) -> Run {
    Rc::new(move |program: String, args: Vec<String>, _: Option<String>| {
        calls.borrow_mut().push(std::iter::once(program).chain(args.iter().cloned()).collect());
        let flag = |name: &str| args.iter().position(|arg| arg == name).map(|at| PathBuf::from(&args[at + 1])).unwrap();
        let applied = args.iter().any(|arg| arg == "--apply");
        if applied {
            let (scripts, state, package) = (flag("--remote-scripts-dir"), flag("--state-dir"), flag("--package-root"));
            let script = scripts.join("AbletonMcpBridge");
            std::fs::create_dir_all(&script).unwrap();
            std::fs::write(script.join("__init__.py"), b"# Kumi's Remote Script\n").unwrap();
            std::fs::create_dir_all(&state).unwrap();
            installed_config(&script, &state, &package.join(executable_name("ableton-mcp-server")));
        }
        let state = if applied { "completed" } else { "planned" };
        let said =
            Ran { code: 0, stdout: format!("{}\n", json!({"version": "ableton-mcp-lifecycle/v1", "state": state})), stderr: String::new() };
        async move { said }.boxed_local()
    })
}

/// A bridge configuration in `state` starting `server`, and the Remote Script's reference to it.
fn installed_config(script: &Path, state: &Path, server: &Path) -> PathBuf {
    let config = state.join("bridge-config.json");
    let body =
        json!({"version": 2, "server": {"command": server, "args": ["--config", config]}, "bridge": {"host": "127.0.0.1", "port": 9765}});
    std::fs::write(&config, body.to_string()).unwrap();
    std::fs::create_dir_all(script).unwrap();
    std::fs::write(script.join("bridge-reference.json"), json!({"config": config}).to_string()).unwrap();
    config
}

/// A Live bridge `home` put in at `version`: its package in `<home>/bridge/<version>-1/package`, its configuration in
/// `config_dir` (an earlier install's own folder, or `<home>/bridge/state`).
fn installed_by(home: &Path, version: &str, scripts: &Path, config_dir: &Path) {
    let package = home.join(format!("bridge/{version}-1/package"));
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("package.json"), json!({"version": version}).to_string()).unwrap();
    let server = package.join(executable_name("ableton-mcp-server"));
    std::fs::write(&server, b"the bridge\n").unwrap();
    std::fs::create_dir_all(config_dir).unwrap();
    installed_config(&scripts.join("AbletonMcpBridge"), config_dir, &server);
}

struct Lab {
    root: tempfile::TempDir,
    ctx: Rc<Ctx>,
    home: PathBuf,
    calls: Rc<RefCell<Vec<Vec<String>>>>,
    live_open: Rc<Cell<bool>>,
}

impl Lab {
    /// A runtime for `daw`, its Kumi home the plugin's own (`<root>/plugin/kumi-home`), or one setup adopted
    /// (`<root>/standalone/.kumi`). Live's User Library has its Remote Scripts folder; Renoise 3.5.4's preferences are there.
    fn new(daw: DawId, adopted: bool) -> Lab {
        Lab::with(daw, adopted, false)
    }

    /// [`Lab::new`]; with `real`, Live's bridge is a strict release package installed by the bridge's own lifecycle.
    /// REAPER's extension and Renoise's tool come from a release-laid-out bundle (`<root>/runtime/daws/…`).
    fn with(daw: DawId, adopted: bool, real: bool) -> Lab {
        let root = tempfile::tempdir().unwrap();
        let home = if adopted { at(root.path(), "standalone/.kumi") } else { at(root.path(), "plugin/kumi-home") };
        std::fs::create_dir_all(at(root.path(), "User Library/Remote Scripts")).unwrap();
        std::fs::create_dir_all(at(root.path(), "Renoise/V3.5.4/Scripts/Tools")).unwrap();
        std::fs::create_dir_all(at(root.path(), "REAPER/UserPlugins")).unwrap();
        if adopted {
            std::fs::create_dir_all(&home).unwrap();
        }
        let env = env_for(root.path(), &home, &root.path().join("Renoise"));
        let calls = Rc::new(RefCell::new(vec![]));
        let live_open = Rc::new(Cell::new(false));
        let open = live_open.clone();
        let (prepared, run) =
            if real { (release(root.path()), real_lifecycle(calls.clone())) } else { (bundle(root.path()), lifecycle(calls.clone())) };
        let answers = kumi_daw::io::Answers::default()
            .with(PreparedBridge(prepared.display().to_string()))
            .with(CarriedBy(carried(root.path(), "runtime", None, b"the extension")))
            .with(LiveRunning(Rc::new(move || {
                let open = open.get();
                async move { open }.boxed_local()
            })))
            .with(run);
        let ctx = runtime(root.path(), "plugin/state", &home, daw, env, answers);
        Lab { root, ctx, home, calls, live_open }
    }
}

/// The environment a runtime for the Kumi home `home` reads, with Renoise's preferences in `renoise` (its folder, or one
/// version folder in it).
fn env_for(root: &Path, home: &Path, renoise: &Path) -> Env {
    Env::from([
        ("HOME".into(), root.display().to_string()),
        ("KUMI_HOME".into(), home.display().to_string()),
        ("KUMI_NO_UPDATE_CHECK".into(), "1".into()),
        ("KUMI_PROCESSES".into(), "none".into()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), at(root, "User Library/Remote Scripts").display().to_string()),
        ("KUMI_RENOISE_PREFS_DIR".into(), renoise.display().to_string()),
        ("KUMI_REAPER_RESOURCE_DIR".into(), root.join("REAPER").display().to_string()),
    ])
}

/// A runtime for `daw` over the Kumi home `home`, its state in `<root>/<state>`.
fn runtime(root: &Path, state: &str, home: &Path, daw: DawId, env: Env, answers: kumi_daw::io::Answers) -> Rc<Ctx> {
    let args = Args {
        state_dir: root.join(state),
        kumi_home: home.to_path_buf(),
        daw,
        library_roots: vec![],
        media_roots: vec![],
        tools: BTreeMap::new(),
    };
    Ctx::new(args, env, answers).unwrap()
}

/// A folder copied whole, as a DAW copies its own (Renoise's newer version folder takes the tools of the one before).
fn copy_folder(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (from, to) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().unwrap().is_dir() {
            copy_folder(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// A release's folder (`<root>/<name>`) carrying REAPER's extension (these bytes, at the version Renoise's tool has)
/// and this checkout's Renoise tool, saying `tool` as its version when given: laid out as a release lays them out.
fn carried(root: &Path, name: &str, tool: Option<&str>, extension: &[u8]) -> PathBuf {
    let app = root.join(name);
    let folder = at(&app, "daws/renoise/com.kumi.Bridge.xrnx");
    copy_folder(&kumi_testkit::workspace_path("daws/renoise/com.kumi.Bridge.xrnx"), &folder);
    let lua = at(&folder, "payload/version.lua");
    let text = std::fs::read_to_string(&lua).unwrap();
    let version = text.split('"').nth(1).unwrap().to_string();
    if let Some(tool) = tool {
        std::fs::write(&lua, text.replace(&format!("\"{version}\""), &format!("\"{tool}\""))).unwrap();
    }
    let reaper = at(&app, "daws/reaper");
    std::fs::create_dir_all(&reaper).unwrap();
    std::fs::write(reaper.join(daw_bridge::install::reaper_extension_name(daw_bridge::detect::Os::current())), extension).unwrap();
    std::fs::write(reaper.join("reaper_kumi.version"), format!("{}\n", tool.unwrap_or(&version))).unwrap();
    app
}

impl Lab {
    fn path(&self, path: &str) -> PathBuf {
        at(self.root.path(), path)
    }

    fn scripts(&self) -> PathBuf {
        self.path("User Library/Remote Scripts")
    }

    fn ears(&self) -> PathBuf {
        self.path("User Library/Kumi/Audio Effects/Kumi Ears.amxd")
    }

    fn lease(&self) -> PathBuf {
        self.home.join("serve").join(format!("{}.lease", self.ctx.daw().as_str()))
    }

    fn tree(&self) -> Tree {
        tree(self.root.path())
    }

    async fn setup(&self, dry_run: bool) -> Result<Value, OpError> {
        setup_on(&self.ctx, dry_run).await
    }

    /// A real install of the plan whose digest is `expected` (the one approved).
    async fn install(&self, expected: &Value) -> Result<Value, OpError> {
        call(&self.ctx, json!({"daw": self.ctx.daw().as_str(), "dry_run": false, "expected": expected})).await
    }
}

/// The runtime's `setup_daw` for its own DAW: a dry run, or a real install of what a dry run plans just before (the
/// plan named by its digest, as setup names the one approved).
async fn setup_on(ctx: &Ctx, dry_run: bool) -> Result<Value, OpError> {
    let daw = ctx.daw().as_str();
    let plan = call(ctx, json!({"daw": daw, "dry_run": true})).await;
    if dry_run {
        return plan;
    }
    call(ctx, json!({"daw": daw, "dry_run": false, "expected": plan?["digest"]})).await
}

async fn call(ctx: &Ctx, args: Value) -> Result<Value, OpError> {
    let operation = registry::OPERATIONS.iter().find(|operation| operation.name == "setup_daw").unwrap();
    let Value::Object(args) = args else { unreachable!() };
    (operation.run)(ctx, Call { args, host: HostCall::default(), signal: Signal::new(), progress: Progress::none() }).await
}

fn refused(result: Result<Value, OpError>) -> (&'static str, Option<Value>) {
    match result {
        Err(OpError::Refused { code, owner, .. }) => (code, owner),
        other => panic!("not refused: {other:?}"),
    }
}

fn strings(values: &[&Path]) -> Value {
    json!(values.iter().map(|path| path.display().to_string()).collect::<Vec<_>>())
}

case!(setup_daw_plans_lives_part_from_files_alone_and_writes_nothing, async {
    let lab = Lab::new(DawId::Ableton, false);
    let before = lab.tree();
    let plan = lab.setup(true).await.unwrap();
    assert_eq!(lab.tree(), before, "a dry run writes nothing, not even the lease's file");
    assert!(lab.calls.borrow().is_empty(), "and runs nothing");
    let script = lab.scripts().join("AbletonMcpBridge");
    assert_eq!(plan["paths"], strings(&[&script, &lab.home.join("bridge"), &lab.ears()]));
    assert_eq!((plan["state"].as_str(), plan["after"].as_str()), (Some("missing"), Some("choose_control_surface")));
    assert_eq!((plan["component"].as_str(), plan["version"].as_str()), (Some("Live Remote Script"), Some(bundled().as_str())));
    assert_eq!((&plan["owner"], &plan["keep_identity"]), (&Value::Null, &json!(false)));
    assert_eq!(
        plan["parts"],
        json!([
            {"name": "Remote Script", "path": script.display().to_string(), "state": "missing"},
            {"name": "Kumi Ears", "path": lab.ears().display().to_string(), "state": "missing"},
        ])
    );
    let effects: Vec<&str> = plan["effects"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for path in plan["paths"].as_array().unwrap().iter().filter_map(Value::as_str) {
        assert!(effects.iter().any(|effect| effect.contains(path)), "an effect names {path}: {effects:?}");
    }
    assert!(effects.iter().any(|effect| effect.contains("Live must be closed")), "{effects:?}");
});

case!(setup_daw_puts_lives_part_in_exactly_as_planned_and_again_changes_nothing, async {
    let lab = Lab::new(DawId::Ableton, false);
    let plan = lab.setup(true).await.unwrap();
    let before = lab.tree();
    let done = lab.setup(false).await.unwrap();
    within(&differ(&before, &lab.tree()), &plan, &lab.lease());
    assert_eq!((done["state"].as_str(), &done["paths"], &done["after"]), (Some("same"), &plan["paths"], &plan["after"]), "{done}");
    assert_eq!((done["parts"][0]["state"].as_str(), done["parts"][1]["state"].as_str()), (Some("same"), Some("same")));
    assert!(done["installed_at"].as_i64().is_some_and(|at| at > 0));
    assert_eq!(std::fs::read(lab.ears()).unwrap(), kumi_daw_ableton::ears::device::ears_file(), "Kumi Ears is in place");
    {
        let calls = lab.calls.borrow();
        assert_eq!(calls.len(), 2, "the lifecycle plans, then applies: {calls:?}");
        let flag = |call: &Vec<String>, name: &str| call.iter().position(|arg| arg == name).map(|at| call[at + 1].clone()).unwrap();
        for call in calls.iter() {
            assert_eq!(call[1..3], ["lifecycle".to_string(), "install".to_string()]);
            assert_eq!(flag(call, "--remote-scripts-dir"), lab.scripts().display().to_string());
            assert_eq!(flag(call, "--state-dir"), at(&lab.home, "bridge/state").display().to_string());
            let package = PathBuf::from(flag(call, "--package-root"));
            assert!(package.starts_with(lab.home.join("bridge")) && package.ends_with("package"), "{}", package.display());
            assert!(package.join("package.json").is_file(), "the bundle's tarball unpacked in a generation of the home's own");
        }
        assert!(!calls[0].contains(&"--apply".to_string()));
        assert!(calls[1].ends_with(&["--apply".to_string(), "--confirm-live-stopped".to_string()]));
    }
    let before = lab.tree();
    let again = lab.setup(false).await.unwrap();
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
    assert_eq!(lab.calls.borrow().len(), 2, "nor run");
    assert_eq!((again["state"].as_str(), again.get("installed_at")), (Some("same"), None));
});

case!(setup_daw_leaves_another_kumis_part_in_live_as_it_is_in_either_layout, async {
    for earlier in [false, true] {
        let lab = Lab::new(DawId::Ableton, false);
        let other = lab.path("standalone/.kumi");
        let config_dir = if earlier { lab.path(".config/ableton-mcp-release-a8a291e") } else { at(&other, "bridge/state") };
        installed_by(&other, &bundled(), &lab.scripts(), &config_dir);
        let plan = lab.setup(true).await.unwrap();
        let owner = json!({"kumi_home": std::fs::canonicalize(&other).unwrap().display().to_string()});
        assert_eq!((plan["state"].as_str(), &plan["owner"], &plan["paths"]), (Some("other_owner"), &owner, &json!([])), "{plan}");
        assert_eq!(plan["parts"][1]["state"], "missing", "Kumi Ears follows the Remote Script's owner: it isn't written either");
        let before = lab.tree();
        assert_eq!(refused(lab.setup(false).await), ("owned_elsewhere", Some(owner)));
        within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
        assert!(lab.calls.borrow().is_empty());
    }
});

case!(setup_daw_never_puts_an_older_bridge_in_live_over_a_newer_one, async {
    let lab = Lab::new(DawId::Ableton, false);
    installed_by(&lab.home, "99.0.0", &lab.scripts(), &at(&lab.home, "bridge/state"));
    let plan = lab.setup(true).await.unwrap();
    assert_eq!((plan["state"].as_str(), plan["version"].as_str(), &plan["paths"]), (Some("newer"), Some("99.0.0"), &json!([])));
    let before = lab.tree();
    assert_eq!(refused(lab.setup(false).await).0, "newer_installed");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
});

case!(setup_daw_reports_an_adopted_installation_and_never_writes_into_it, async {
    let lab = Lab::new(DawId::Ableton, true);
    installed_by(&lab.home, &bundled(), &lab.scripts(), &at(&lab.home, "bridge/state"));
    let plan = lab.setup(true).await.unwrap();
    assert_eq!((plan["state"].as_str(), &plan["paths"]), (Some("missing"), &json!([])), "{plan}");
    assert_eq!(plan["parts"][0]["state"], "same");
    assert_eq!(plan["parts"][1]["state"], "missing", "a missing Kumi Ears is reported");
    assert!(plan["effects"].to_string().contains("--daw ableton bridge"), "with the fix, that Kumi's own: {}", plan["effects"]);
    let before = lab.tree();
    assert_eq!(refused(lab.setup(false).await).0, "adopted_home");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
    assert!(!lab.ears().exists());
});

case!(setup_daw_wont_change_lives_remote_script_while_live_is_open, async {
    let lab = Lab::new(DawId::Ableton, false);
    lab.live_open.set(true);
    let before = lab.tree();
    assert_eq!(refused(lab.setup(false).await).0, "in_use");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
    assert!(lab.calls.borrow().is_empty(), "nothing ran");
    lab.live_open.set(false);
    assert_eq!(lab.setup(false).await.unwrap()["state"], "same", "with Live closed it goes in");
});

case!(setup_daw_sets_up_only_its_own_daw_and_one_runtime_at_a_time, async {
    let lab = Lab::new(DawId::Ableton, false);
    let operation = registry::OPERATIONS.iter().find(|operation| operation.name == "setup_daw").unwrap();
    let Value::Object(args) = json!({"daw": "renoise", "dry_run": true}) else { unreachable!() };
    let call = Call { args, host: HostCall::default(), signal: Signal::new(), progress: Progress::none() };
    assert_eq!(refused((operation.run)(&lab.ctx, call).await).0, "other_daw");
    match self::call(&lab.ctx, json!({"daw": "ableton", "dry_run": false})).await {
        Err(OpError::Invalid { .. }) => {}
        other => panic!("a real install that names no approved plan ran: {other:?}"),
    }
    let other = Lease::new(&lab.home, "ableton");
    let held = other.take(&HostCall::default()).await.unwrap();
    assert_eq!(lab.setup(true).await.unwrap()["state"], "missing", "a dry run takes no lease");
    assert_eq!(refused(lab.setup(false).await).0, "leased");
    drop(held);
    assert_eq!(lab.setup(false).await.unwrap()["state"], "same");
});

case!(setup_daw_puts_renoises_tool_in_exactly_as_planned_and_leaves_another_kumis, async {
    let lab = Lab::new(DawId::Renoise, false);
    let tool = lab.path("Renoise/V3.5.4/Scripts/Tools/com.kumi.Bridge.xrnx");
    let before = lab.tree();
    let plan = lab.setup(true).await.unwrap();
    assert_eq!(lab.tree(), before, "a dry run writes nothing");
    assert_eq!(plan["paths"], strings(&[&tool, &at(&lab.home, "bridges/renoise")]), "{plan}");
    assert_eq!(
        (plan["state"].as_str(), plan["after"].as_str(), plan["component"].as_str()),
        (Some("missing"), Some("restart_daw"), Some("Renoise tool"))
    );
    let done = lab.setup(false).await.unwrap();
    within(&differ(&before, &lab.tree()), &plan, &lab.lease());
    assert_eq!((done["state"].as_str(), &done["paths"]), (Some("same"), &plan["paths"]), "what it wrote, and how it stands: {done}");
    assert!(tool.join("main.lua").is_file());
    let before = lab.tree();
    assert_eq!(lab.setup(false).await.unwrap().get("installed_at"), None);
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());

    // The same Renoise, from another Kumi home: the tool is the first one's, so it stays.
    let home = lab.path("plugin/elsewhere");
    let env = env_for(lab.root.path(), &home, &lab.path("Renoise"));
    let other = runtime(lab.root.path(), "plugin/state-2", &home, DawId::Renoise, env, carrying(&lab, "runtime"));
    let plan = setup_on(&other, true).await.unwrap();
    assert_eq!((plan["state"].as_str(), &plan["paths"]), (Some("other_owner"), &json!([])), "{plan}");
    assert_eq!(plan["owner"], json!({"kumi_home": lab.home.display().to_string()}));
    let before = lab.tree();
    assert_eq!(refused(setup_on(&other, false).await).0, "owned_elsewhere");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &at(&home, "serve/renoise.lease"));
});

case!(setup_daw_puts_lives_part_in_through_the_bridges_own_lifecycle_writing_only_what_it_planned, async {
    let lab = Lab::with(DawId::Ableton, false, true);
    let plan = lab.setup(true).await.unwrap();
    let before = lab.tree();
    let done = lab.install(&plan["digest"]).await.unwrap();
    within(&differ(&before, &lab.tree()), &plan, &lab.lease());
    assert_eq!((done["state"].as_str(), &done["paths"]), (Some("same"), &plan["paths"]), "{done}");
    let script = lab.scripts().join("AbletonMcpBridge");
    assert!(script.join("ableton_mcp_remote_script.py").is_file(), "the release's Remote Script is in Live's folder");
    assert!(script.join("bridge-reference.json").is_file());
    assert!(at(&lab.home, "bridge/state/install-receipt.json").is_file(), "the lifecycle's receipt is in the home's bridge folder");
    assert_eq!(std::fs::read(lab.ears()).unwrap(), kumi_daw_ableton::ears::device::ears_file());
    let again = lab.setup(true).await.unwrap();
    assert_eq!((again["state"].as_str(), &again["paths"]), (Some("same"), &json!([])), "{again}");
});

case!(setup_daw_installs_only_the_plan_that_was_approved_and_never_another_kumis_ears, async {
    let lab = Lab::new(DawId::Ableton, false);
    let approved = lab.setup(true).await.unwrap();
    // Before the install, another Kumi's Kumi Ears (a newer one, perhaps) turns up in the User Library.
    std::fs::create_dir_all(lab.ears().parent().unwrap()).unwrap();
    std::fs::write(lab.ears(), b"another Kumi's Kumi Ears").unwrap();
    let before = lab.tree();
    assert_eq!(refused(lab.install(&approved["digest"]).await).0, "changed", "what's there isn't what was approved");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
    assert!(lab.calls.borrow().is_empty(), "nothing ran");
    let plan = lab.setup(true).await.unwrap();
    assert_eq!(plan["parts"][1]["state"], "other_owner", "with no Remote Script of this home's, it isn't this Kumi's to replace");
    assert!(!plan["paths"].to_string().contains("Kumi Ears.amxd"), "{plan}");
    assert!(plan["effects"].to_string().contains("isn't this Kumi's copy: it stays as it is"), "{}", plan["effects"]);
    lab.install(&plan["digest"]).await.unwrap();
    assert_eq!(std::fs::read(lab.ears()).unwrap(), b"another Kumi's Kumi Ears", "it stays as it is");
});

case!(setup_daw_names_the_folders_an_earlier_install_keeps_its_bridge_settings_in, async {
    let lab = Lab::new(DawId::Ableton, false);
    let earlier = lab.path(".config/ableton-mcp-release-a8a291e");
    installed_by(&lab.home, "0.9.0", &lab.scripts(), &earlier);
    let plan = lab.setup(true).await.unwrap();
    assert_eq!(plan["parts"][0]["state"], "older", "{plan}");
    let paths: Vec<&str> = plan["paths"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(paths.contains(&earlier.display().to_string().as_str()), "the update writes there too: {paths:?}");
    let effects: Vec<&str> = plan["effects"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(effects.iter().any(|effect| effect.contains(&earlier.display().to_string())), "{effects:?}");
    let before = lab.tree();
    lab.install(&plan["digest"]).await.unwrap();
    within(&differ(&before, &lab.tree()), &plan, &lab.lease());
    let calls = lab.calls.borrow();
    let state = calls[1].iter().position(|arg| arg == "--state-dir").map(|at| calls[1][at + 1].clone()).unwrap();
    assert_eq!(state, earlier.display().to_string(), "the lifecycle is given the folder the plan named");
});

/// Answers for another runtime over the same root: the release folder `name` carries its REAPER and Renoise parts.
fn carrying(lab: &Lab, name: &str) -> kumi_daw::io::Answers {
    kumi_daw::io::Answers::default().with(CarriedBy(lab.path(name)))
}

case!(setup_daw_puts_reapers_extension_in_exactly_as_planned_and_never_into_an_adopted_home, async {
    let lab = Lab::new(DawId::Reaper, false);
    let extension = lab.path("REAPER/UserPlugins").join(daw_bridge::install::reaper_extension_name(daw_bridge::detect::Os::current()));
    let reference = lab.path("REAPER/UserPlugins/kumi-bridge-reference.json");
    let before = lab.tree();
    let plan = lab.setup(true).await.unwrap();
    assert_eq!(lab.tree(), before, "a dry run writes nothing");
    assert_eq!(plan["paths"], strings(&[&extension, &reference, &at(&lab.home, "bridges/reaper")]), "{plan}");
    assert_eq!((plan["state"].as_str(), plan["component"].as_str()), (Some("missing"), Some("REAPER extension")));
    let done = lab.install(&plan["digest"]).await.unwrap();
    within(&differ(&before, &lab.tree()), &plan, &lab.lease());
    assert_eq!((done["state"].as_str(), done["after"].as_str()), (Some("same"), Some("restart_daw")), "{done}");
    assert_eq!(std::fs::read(&extension).unwrap(), b"the extension");

    let adopted = Lab::new(DawId::Reaper, true);
    let before = adopted.tree();
    assert_eq!(refused(adopted.setup(false).await).0, "adopted_home");
    assert_eq!(adopted.tree(), before, "nothing is written, not even the lease's file");
});

case!(setup_daw_takes_reapers_and_renoises_parts_only_from_the_runtimes_release_bundle, async {
    let lab = Lab::new(DawId::Renoise, false);
    let env = env_for(lab.root.path(), &lab.home, &lab.path("Renoise"));
    // A runtime with nothing laid out as a release lays it out: never this checkout's build.
    let bare = runtime(lab.root.path(), "plugin/state-2", &lab.home, DawId::Renoise, env, kumi_daw::io::Answers::default());
    match setup_on(&bare, true).await {
        Err(OpError::Failed { reason }) => assert!(reason.contains("doesn't carry"), "{reason}"),
        other => panic!("planned without a bundle: {other:?}"),
    }
});

case!(setup_daw_leaves_another_homes_copy_in_another_renoise_folder_alone, async {
    let lab = Lab::new(DawId::Renoise, false);
    // Another Kumi home put its tool in Renoise 3.5.4's folder; Renoise 3.5.5 has run since, with no tool yet.
    let other = lab.path("standalone/.kumi");
    let env = env_for(lab.root.path(), &other, &lab.path("Renoise/V3.5.4"));
    let theirs = runtime(lab.root.path(), "plugin/state-2", &other, DawId::Renoise, env, carrying(&lab, "runtime"));
    let plan = setup_on(&theirs, true).await.unwrap();
    call(&theirs, json!({"daw": "renoise", "dry_run": false, "expected": plan["digest"]})).await.unwrap();
    let old = lab.path("Renoise/V3.5.4/Scripts/Tools/com.kumi.Bridge.xrnx");
    let reference = std::fs::read(old.join("bridge-reference.json")).unwrap();
    std::fs::create_dir_all(lab.path("Renoise/V3.5.5/Scripts/Tools")).unwrap();
    let tool = lab.path("Renoise/V3.5.5/Scripts/Tools/com.kumi.Bridge.xrnx");
    let plan = lab.setup(true).await.unwrap();
    assert_eq!(plan["paths"], strings(&[&tool, &at(&lab.home, "bridges/renoise")]), "an equal payload isn't this home's: {plan}");
    let before = lab.tree();
    lab.install(&plan["digest"]).await.unwrap();
    within(&differ(&before, &lab.tree()), &plan, &lab.lease());
    assert_eq!(std::fs::read(old.join("bridge-reference.json")).unwrap(), reference, "the other home's copy still names it");
});

case!(setup_daw_never_downgrades_a_tool_renoise_carried_into_its_newer_folder, async {
    let lab = Lab::new(DawId::Renoise, false);
    // This home put in a newer tool (99.0.0) in Renoise 3.5.4's folder, and Renoise 3.5.5 copied it into its own.
    carried(lab.root.path(), "newer", Some("99.0.0"), b"the extension");
    let env = env_for(lab.root.path(), &lab.home, &lab.path("Renoise/V3.5.4"));
    let newer = runtime(lab.root.path(), "plugin/state-2", &lab.home, DawId::Renoise, env, carrying(&lab, "newer"));
    let plan = setup_on(&newer, true).await.unwrap();
    call(&newer, json!({"daw": "renoise", "dry_run": false, "expected": plan["digest"]})).await.unwrap();
    let tools = lab.path("Renoise/V3.5.5/Scripts/Tools");
    copy_folder(&lab.path("Renoise/V3.5.4/Scripts/Tools/com.kumi.Bridge.xrnx"), &tools.join("com.kumi.Bridge.xrnx"));
    let plan = lab.setup(true).await.unwrap();
    assert_eq!((plan["state"].as_str(), plan["version"].as_str(), &plan["paths"]), (Some("newer"), Some("99.0.0"), &json!([])), "{plan}");
    let before = lab.tree();
    assert_eq!(refused(lab.install(&plan["digest"]).await).0, "newer_installed");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
});

case!(setup_daw_puts_back_what_renoises_tool_needs_and_clears_what_an_interrupted_install_left, async {
    let lab = Lab::new(DawId::Renoise, false);
    let tools = lab.path("Renoise/V3.5.4/Scripts/Tools");
    // An install cut short left an old copy aside.
    let leftover = tools.join("com.kumi.Bridge.xrnx.kumi-old-0123456789ab");
    std::fs::create_dir_all(&leftover).unwrap();
    std::fs::write(leftover.join("main.lua"), b"-- left\n").unwrap();
    let plan = lab.setup(true).await.unwrap();
    let paths: Vec<&str> = plan["paths"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(paths.contains(&leftover.display().to_string().as_str()), "what it removes is named: {paths:?}");
    let before = lab.tree();
    lab.install(&plan["digest"]).await.unwrap();
    within(&differ(&before, &lab.tree()), &plan, &lab.lease());
    assert!(!leftover.exists());
    // The tool loses where it finds Kumi: setup puts that back without replacing the tool.
    let reference = at(&tools, "com.kumi.Bridge.xrnx/bridge-reference.json");
    std::fs::remove_file(&reference).unwrap();
    let plan = lab.setup(true).await.unwrap();
    assert_eq!(plan["state"], "older", "a tool that can't reach Kumi isn't the same: {plan}");
    let paths: Vec<&str> = plan["paths"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(paths.contains(&reference.display().to_string().as_str()), "{paths:?}");
    lab.install(&plan["digest"]).await.unwrap();
    assert!(reference.is_file());
    assert_eq!(lab.setup(true).await.unwrap()["state"], "same");
});

#[cfg(windows)]
case!(setup_daw_wont_replace_reapers_extension_while_reaper_holds_it_open, async {
    use std::os::windows::fs::OpenOptionsExt;
    let lab = Lab::new(DawId::Reaper, false);
    let plan = lab.setup(true).await.unwrap();
    lab.install(&plan["digest"]).await.unwrap();
    // A newer runtime brings other bytes at the same version: an update, while REAPER has the DLL loaded.
    let extension = lab.path("REAPER/UserPlugins").join(daw_bridge::install::reaper_extension_name(daw_bridge::detect::Os::current()));
    carried(lab.root.path(), "update", None, b"the updated extension");
    let env = env_for(lab.root.path(), &lab.home, &lab.path("Renoise"));
    let update = runtime(lab.root.path(), "plugin/state-2", &lab.home, DawId::Reaper, env, carrying(&lab, "update"));
    let plan = setup_on(&update, true).await.unwrap();
    assert_eq!(plan["state"], "older", "{plan}");
    // (Taken before the DLL is held: a held file can't be read either.)
    let before = lab.tree();
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&extension).unwrap();
    match call(&update, json!({"daw": "reaper", "dry_run": false, "expected": plan["digest"]})).await {
        Err(OpError::Refused { code: "in_use", .. }) => {}
        other => panic!("replaced while held: {other:?}"),
    }
    drop(held);
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
});
