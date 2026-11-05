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

/// Every file, folder and link under a folder (a file by its bytes, a folder by a mark, a link by the path it names):
/// what a step wrote or made is what differs. The runtimes' own state folders (`plugin/state…`, where their snapshots
/// are written as they go) are left out.
type Tree = BTreeMap<PathBuf, Vec<u8>>;

fn tree(root: &Path) -> Tree {
    let mut found = Tree::new();
    let mut folders = vec![root.to_path_buf()];
    let runtimes = root.join("plugin");
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap().flatten() {
            let (path, kind) = (entry.path(), entry.file_type().unwrap());
            if kind.is_dir() && folder == runtimes && entry.file_name().to_string_lossy().starts_with("state") {
                continue;
            } else if kind.is_dir() {
                found.insert(path.clone(), b"<folder>".to_vec());
                folders.push(path);
            } else if kind.is_symlink() {
                found.insert(path.clone(), std::fs::read_link(&path).unwrap().display().to_string().into_bytes());
            } else {
                found.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    found
}

/// What was added, changed or gone between two looks.
fn differ(before: &Tree, after: &Tree) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = before.keys().chain(after.keys()).filter(|path| before.get(*path) != after.get(*path)).cloned().collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Each file or folder a step wrote, made or removed lies in one of the paths its plan named, or is the lease's own
/// file, or a folder of the Kumi home's on the way to one of those (the home's own folders, which plans leave out).
fn within(written: &[PathBuf], plan: &Value, lease: &Path) {
    let named: Vec<PathBuf> = plan["paths"].as_array().unwrap().iter().map(|path| PathBuf::from(path.as_str().unwrap())).collect();
    let home = lease.parent().and_then(Path::parent).unwrap();
    for path in written {
        let on_the_way = path.starts_with(home) && (lease.starts_with(path) || named.iter().any(|named| named.starts_with(path)));
        assert!(
            path == lease || on_the_way || named.iter().any(|named| path.starts_with(named)),
            "{} isn't in the plan's {named:?}",
            path.display()
        );
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

/// A tar archive of `files` by their paths in it, each a regular file with its mode, as a release writes one.
fn tar(files: &BTreeMap<String, (Vec<u8>, u32)>) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, (content, mode)) in files {
        let mut header = [0u8; 512];
        let mut field = |offset: usize, size: usize, value: &[u8]| {
            header[offset..offset + value.len().min(size)].copy_from_slice(&value[..value.len().min(size)]);
        };
        field(0, 100, name.as_bytes());
        field(100, 8, format!("{mode:07o}\0").as_bytes());
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

/// A strict native release package of Live's bridge at `version` (its manifest binding every file, the Remote Script
/// among them, the programs executable), as a release makes it: its tarball (`<dir>/bridge.tgz`, which holds
/// `package/`), and, with `unpacked`, the package itself in `<dir>/package`. The tarball's SHA-256.
fn release_package(dir: &Path, version: &str, unpacked: bool) -> String {
    use std::io::Write;
    let (binary, worker) = (executable_name("ableton-mcp-server"), executable_name("ableton-mcp-analysis-worker"));
    let target = if cfg!(windows) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(target_os = "macos") {
        "aarch64-apple-darwin"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let metadata = json!({"name": "@ableton-mcp/mcp-server", "version": version, "private": true, "license": "MIT", "type": "module",
                          "runtime": "rust-native", "target": target, "bin": {"ableton-mcp-server": binary, "ableton-mcp-analysis-worker": worker}});
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    files.insert("package.json".into(), metadata.to_string().into_bytes());
    files.insert("LICENSE.md".into(), include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/LICENSE.md")).to_vec());
    files.insert(binary.clone(), format!("the bridge {version}\n").into_bytes());
    files.insert(worker.clone(), b"the analysis worker\n".to_vec());
    files.insert("remote-script/AbletonMcpBridge/__init__.py".into(), b"def create_instance(c_instance):\n    return None\n".to_vec());
    files.insert("remote-script/AbletonMcpBridge/ableton_mcp_remote_script.py".into(), format!("marker = '{version}'\n").into_bytes());
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
    // The programs are executable, as a release's are; everything else is the owner's to read and write.
    let mode = |name: &str| if name == binary || name == worker { 0o755 } else { 0o644 };
    let packed: BTreeMap<String, (Vec<u8>, u32)> =
        files.iter().map(|(name, bytes)| (format!("package/{name}"), (bytes.clone(), mode(name)))).collect();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&tar(&packed)).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("bridge.tgz"), gzip.finish().unwrap()).unwrap();
    if unpacked {
        for (name, bytes) in &files {
            let path = at(&dir.join("package"), name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode(name))).unwrap();
            }
        }
    }
    sha256(&dir.join("bridge.tgz"))
}

/// A runtime's bundle of Live's bridge as a release lays it out: a strict release package at this Kumi's bundled version,
/// in the tarball `prepared.json` names. The bridge's own lifecycle installs it as it would a release's.
fn release(root: &Path) -> PathBuf {
    let prepared = at(root, "runtime/bridge");
    let sha = release_package(&prepared, &bundled(), false);
    std::fs::write(prepared.join("prepared.json"), json!({"artifact": "bridge.tgz", "sha256": sha}).to_string()).unwrap();
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
        hermes_home: None,
        hermes_root: None,
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
    // Kumi Ears' folders aren't in the User Library yet: the outermost one it makes is named too.
    let made = lab.path("User Library/Kumi");
    assert_eq!(plan["paths"], strings(&[&script, &lab.home.join("bridge"), &lab.ears(), &lab.home.join("kumi-ears.json"), &made]));
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
    // Every folder the lifecycle was given is one the plan named.
    let named: Vec<PathBuf> = plan["paths"].as_array().unwrap().iter().filter_map(Value::as_str).map(PathBuf::from).collect();
    for call in lab.calls.borrow().iter() {
        for flag in ["--state-dir", "--remote-scripts-dir", "--config"] {
            if let Some(at) = call.iter().position(|arg| arg == flag) {
                let given = Path::new(&call[at + 1]);
                let folder = if flag == "--config" { given.parent().unwrap() } else { given };
                assert!(
                    named.iter().any(|path| folder.starts_with(path)) || named.iter().any(|path| path.starts_with(folder)),
                    "{flag} {}",
                    folder.display()
                );
            }
        }
    }
    let script = lab.scripts().join("AbletonMcpBridge");
    assert!(script.join("ableton_mcp_remote_script.py").is_file(), "the release's Remote Script is in Live's folder");
    assert!(script.join("bridge-reference.json").is_file());
    assert!(at(&lab.home, "bridge/state/install-receipt.json").is_file(), "the lifecycle's receipt is in the home's bridge folder");
    assert_eq!(std::fs::read(lab.ears()).unwrap(), kumi_daw_ableton::ears::device::ears_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let package =
            PathBuf::from(&lab.calls.borrow()[1][lab.calls.borrow()[1].iter().position(|arg| arg == "--package-root").unwrap() + 1]);
        let mode = std::fs::metadata(package.join("ableton-mcp-server")).unwrap().permissions().mode();
        assert_ne!(mode & 0o111, 0, "the server unpacked from the release can run: {mode:o}");
    }
    let again = lab.setup(true).await.unwrap();
    assert_eq!((again["state"].as_str(), &again["paths"]), (Some("same"), &json!([])), "{again}");
});

case!(setup_daw_installs_only_the_plan_that_was_approved_and_never_another_kumis_ears, async {
    let lab = Lab::new(DawId::Ableton, false);
    let approved = lab.setup(true).await.unwrap();
    // Before the install, another Kumi's Kumi Ears (a newer one, perhaps) turns up in the User Library, and an earlier
    // Kumi's in its old place.
    std::fs::create_dir_all(lab.ears().parent().unwrap()).unwrap();
    std::fs::write(lab.ears(), b"another Kumi's Kumi Ears").unwrap();
    let legacy = lab.path("User Library/Kumi/Kumi Ears.amxd");
    let mut earlier = kumi_daw_ableton::ears::device::ears_file();
    earlier.extend_from_slice(b"\n");
    std::fs::write(&legacy, &earlier).unwrap();
    let before = lab.tree();
    assert_eq!(refused(lab.install(&approved["digest"]).await).0, "changed", "what's there isn't what was approved");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
    assert!(lab.calls.borrow().is_empty(), "nothing ran");
    for round in ["the Remote Script goes in", "it's in: this home's now"] {
        let plan = lab.setup(true).await.unwrap();
        assert_eq!(plan["parts"][1]["state"], "other_owner", "{round}: a Kumi Ears this home didn't put there isn't its to replace");
        assert!(!plan["paths"].to_string().contains("Kumi Ears.amxd"), "{round}: {plan}");
        let effects: Vec<&str> = plan["effects"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(effects.iter().any(|effect| effect.contains("isn't one this Kumi put there: it stays as it is")), "{round}: {effects:?}");
        lab.install(&plan["digest"]).await.unwrap();
        assert_eq!(std::fs::read(lab.ears()).unwrap(), b"another Kumi's Kumi Ears", "{round}: it stays as it is");
        assert_eq!(std::fs::read(&legacy).unwrap(), earlier, "{round}: the earlier copy stays too");
    }
});

case!(setup_daw_updates_a_kumi_ears_this_home_put_there_and_never_an_earlier_kumis, async {
    let lab = Lab::new(DawId::Ableton, false);
    // An earlier Kumi's device in its old place: Kumi's own, as `kumi bridge` would remove it. Setup leaves it.
    let legacy = lab.path("User Library/Kumi/Kumi Ears.amxd");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, kumi_daw_ableton::ears::device::ears_file()).unwrap();
    let plan = lab.setup(true).await.unwrap();
    assert_eq!(plan["parts"][1]["state"], "missing", "{plan}");
    let before = lab.tree();
    lab.install(&plan["digest"]).await.unwrap();
    within(&differ(&before, &lab.tree()), &plan, &lab.lease());
    assert_eq!(std::fs::read(lab.ears()).unwrap(), kumi_daw_ableton::ears::device::ears_file(), "installed");
    assert_eq!(std::fs::read(&legacy).unwrap(), kumi_daw_ableton::ears::device::ears_file(), "the earlier copy stays");
    // The device this home wrote, as an earlier Kumi made it: its record names those bytes.
    let old = b"Kumi Ears as an earlier Kumi made it".to_vec();
    std::fs::write(lab.ears(), &old).unwrap();
    let sha = Sha256::digest(&old).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let record = lab.home.join("kumi-ears.json");
    let mut noted: Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    noted["versions"][0] = json!({"sha256": sha, "version": "0.0.0"});
    std::fs::write(&record, noted.to_string()).unwrap();
    let plan = lab.setup(true).await.unwrap();
    assert_eq!(plan["parts"][1]["state"], "older", "{plan}");
    let before = lab.tree();
    lab.install(&plan["digest"]).await.unwrap();
    within(&differ(&before, &lab.tree()), &plan, &lab.lease());
    assert_eq!(std::fs::read(lab.ears()).unwrap(), kumi_daw_ableton::ears::device::ears_file(), "updated");
    assert_eq!(std::fs::read(&legacy).unwrap(), kumi_daw_ableton::ears::device::ears_file(), "the earlier copy still stays");
});

case!(setup_daw_leaves_a_newer_or_unorderable_kumi_ears_in_this_home_as_it_is, async {
    for version in [Some("999.0.0"), Some("1.24.68-dev"), None] {
        let lab = Lab::new(DawId::Ableton, false);
        lab.setup(false).await.unwrap();
        let device = b"Kumi Ears from another runtime sharing this home";
        std::fs::write(lab.ears(), device).unwrap();
        let sha = sha256(&lab.ears());
        let record = lab.home.join("kumi-ears.json");
        let noted = match version {
            Some(version) => json!({"path": lab.ears().display().to_string(), "versions": [{"sha256": sha, "version": version}]}),
            // The original record proved ownership but kept no version: it cannot prove this device is older.
            None => json!({"path": lab.ears().display().to_string(), "sha256": sha}),
        };
        std::fs::write(&record, noted.to_string()).unwrap();
        let before = lab.tree();
        let plan = lab.setup(true).await.unwrap();
        assert_eq!((plan["state"].as_str(), plan["parts"][1]["state"].as_str()), (Some("newer"), Some("newer")), "{plan}");
        assert_eq!(plan["paths"], json!([]), "the same bridge version cannot authorize replacing newer Ears");
        assert_eq!(refused(lab.install(&plan["digest"]).await).0, "newer_installed");
        within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
        assert_eq!(std::fs::read(lab.ears()).unwrap(), device);
    }
});

case!(setup_daw_records_ears_before_replacing_it_and_an_obstructed_record_can_be_retried, async {
    let lab = Lab::new(DawId::Ableton, false);
    let record = lab.home.join("kumi-ears.json");
    std::fs::create_dir_all(&record).unwrap();
    assert!(matches!(lab.setup(false).await, Err(OpError::Failed { .. })));
    assert!(!lab.ears().exists(), "a failed record never leaves an unrecorded device behind");
    assert_eq!(lab.setup(true).await.unwrap()["parts"][0]["state"], "same", "the script was installed before Ears failed");
    std::fs::remove_dir(&record).unwrap();
    assert_eq!(lab.setup(false).await.unwrap()["state"], "same", "retry installs and records Ears");
    let mut noted: Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    assert_eq!(noted["versions"][0]["version"], kumi_types::version::KUMI_VERSION);
    // The retried install's bytes as an earlier runtime's: a later update still knows they're this home's.
    std::fs::write(lab.ears(), b"the retried runtime's earlier Ears").unwrap();
    noted["versions"][0] = json!({"sha256": sha256(&lab.ears()), "version": "0.0.0"});
    std::fs::write(&record, noted.to_string()).unwrap();
    assert_eq!(lab.setup(true).await.unwrap()["parts"][1]["state"], "older");
    assert_eq!(lab.setup(false).await.unwrap()["state"], "same");
    assert_eq!(std::fs::read(lab.ears()).unwrap(), kumi_daw_ableton::ears::device::ears_file());
});

#[cfg(unix)]
case!(setup_daw_leaves_a_kumi_ears_it_cant_read_as_it_is, async {
    use std::os::unix::fs::PermissionsExt;
    let lab = Lab::new(DawId::Ableton, false);
    let plan = lab.setup(true).await.unwrap();
    lab.install(&plan["digest"]).await.unwrap();
    // Another Kumi's device, which this user can't read (its folder is theirs to write).
    std::fs::write(lab.ears(), b"another Kumi's Kumi Ears").unwrap();
    let mode = |mode: u32| std::fs::set_permissions(lab.ears(), std::fs::Permissions::from_mode(mode)).unwrap();
    mode(0o000);
    if std::fs::read(lab.ears()).is_ok() {
        // (root reads it anyway: nothing here to prove)
        return mode(0o644);
    }
    let plan = lab.setup(true).await.unwrap();
    let done = lab.install(&plan["digest"]).await;
    mode(0o644);
    assert_eq!(plan["parts"][1]["state"], "other_owner", "an Ears that can't be read isn't missing: {plan}");
    assert!(!plan["paths"].to_string().contains("Kumi Ears.amxd"), "{plan}");
    assert!(plan["effects"].to_string().contains("can't read the Kumi Ears"), "{}", plan["effects"]);
    done.unwrap();
    assert_eq!(std::fs::read(lab.ears()).unwrap(), b"another Kumi's Kumi Ears", "it stays as it is");
});

case!(setup_daw_upgrades_an_earlier_install_through_the_bridges_lifecycle_in_the_folders_its_receipt_names, async {
    let lab = Lab::with(DawId::Ableton, false, true);
    // An earlier install of this home's: its package in `<home>/bridge/0.9.0-1`, its state and configuration in the
    // earlier layout's own folder, put in by the bridge's own lifecycle (receipt and all).
    let generation = at(&lab.home, "bridge/0.9.0-1");
    let sha = release_package(&generation, "0.9.0", true);
    let earlier = lab.path(".config/ableton-mcp-release-a8a291e");
    let free = || std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string();
    let args: Vec<String> = vec![
        "lifecycle".into(),
        "install".into(),
        "--remote-scripts-dir".into(),
        lab.scripts().display().to_string(),
        "--state-dir".into(),
        earlier.display().to_string(),
        "--package-root".into(),
        generation.join("package").display().to_string(),
        "--artifact".into(),
        generation.join("bridge.tgz").display().to_string(),
        "--artifact-sha256".into(),
        sha,
        "--apply".into(),
        "--confirm-live-stopped".into(),
        "--port".into(),
        free(),
        "--realtime-port".into(),
        free(),
    ];
    let installed = ableton_mcp_server::cli::auxiliary_command(&args).await.unwrap();
    assert_eq!(installed.code, 0, "{}{}", installed.stdout, installed.stderr);
    assert!(earlier.join("install-receipt.json").is_file());
    let plan = lab.setup(true).await.unwrap();
    assert_eq!(plan["parts"][0]["state"], "older", "{plan}");
    let paths: Vec<&str> = plan["paths"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(paths.contains(&earlier.display().to_string().as_str()), "{paths:?}");
    let before = lab.tree();
    let done = lab.install(&plan["digest"]).await.unwrap();
    within(&differ(&before, &lab.tree()), &plan, &lab.lease());
    assert_eq!(done["parts"][0]["state"], "same", "{done}");
    let calls = lab.calls.borrow();
    let flag = |name: &str| calls[1].iter().position(|arg| arg == name).map(|at| calls[1][at + 1].clone()).unwrap();
    assert_eq!((calls[1][2].as_str(), flag("--state-dir")), ("upgrade", earlier.display().to_string()), "the receipt's own folder");
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

/// `bytes` with CC-8's mark in them, as a bridge that keeps a project's id only when asked carries it.
fn marked(bytes: &[u8]) -> Vec<u8> {
    [bytes, daw_bridge::contract::KEEPS_IDENTITY_ONLY_WHEN_ASKED.as_bytes()].concat()
}

case!(setup_daw_says_a_part_keeps_an_id_only_when_asked_from_the_files_of_the_copy_in_once_it_has_run, async {
    let lab = Lab::new(DawId::Reaper, false);
    let env = || env_for(lab.root.path(), &lab.home, &lab.path("Renoise"));
    // Nothing in REAPER yet: the part once in is the extension a runtime brings, so its library decides.
    carried(lab.root.path(), "marked", None, &marked(b"the extension"));
    let bringing = runtime(lab.root.path(), "plugin/state-2", &lab.home, DawId::Reaper, env(), carrying(&lab, "marked"));
    let plan = setup_on(&bringing, true).await.unwrap();
    assert_eq!((plan["state"].as_str(), &plan["keep_identity"]), (Some("missing"), &json!(true)), "{plan}");
    // One without the mark, at the same version, says nothing; and the approval of one isn't the other's.
    let unmarked = lab.setup(true).await.unwrap();
    assert_eq!((&unmarked["keep_identity"], &unmarked["version"]), (&json!(false), &plan["version"]), "{unmarked}");
    let before = lab.tree();
    assert_eq!(refused(lab.install(&plan["digest"]).await).0, "changed");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
    // A copy that stays is the part: another home's extension speaks for itself, whatever the runtime brings.
    let user_plugins = lab.path("REAPER/UserPlugins");
    let name = daw_bridge::install::reaper_extension_name(daw_bridge::detect::Os::current());
    let target = daw_bridge::lifecycle::Target {
        daw: daw_bridge::Daw::Reaper,
        dir: user_plugins.clone(),
        name: name.to_string(),
        folder: false,
        reference: user_plugins.join("kumi-bridge-reference.json"),
    };
    let other = daw_bridge::lifecycle::Lifecycle::new(&lab.path("standalone/.kumi"), target);
    for (theirs, says) in [("runtime", false), ("marked", true)] {
        other.install(&lab.path(theirs).join("daws/reaper").join(name), "1.0.0").unwrap();
        for ctx in [&bringing, &lab.ctx] {
            let plan = setup_on(ctx, true).await.unwrap();
            assert_eq!((plan["state"].as_str(), &plan["keep_identity"]), (Some("other_owner"), &json!(says)), "{theirs}: {plan}");
        }
    }
});

case!(setup_daw_reads_renoises_mark_in_its_tools_status_file, async {
    let lab = Lab::new(DawId::Renoise, false);
    let env = || env_for(lab.root.path(), &lab.home, &lab.path("Renoise"));
    let mark = daw_bridge::contract::KEEPS_IDENTITY_ONLY_WHEN_ASKED;
    for (name, says) in [("marked", true), ("unmarked", false)] {
        let status = carried(lab.root.path(), name, None, b"the extension").join("daws/renoise/com.kumi.Bridge.xrnx/payload/status.lua");
        let text = std::fs::read_to_string(&status).unwrap().replace(mark, "");
        std::fs::write(&status, if says { format!("{text}\n-- {mark}\n") } else { text }).unwrap();
        let bringing = runtime(lab.root.path(), &format!("plugin/state-{name}"), &lab.home, DawId::Renoise, env(), carrying(&lab, name));
        let plan = setup_on(&bringing, true).await.unwrap();
        assert_eq!((plan["state"].as_str(), &plan["keep_identity"]), (Some("missing"), &json!(says)), "{name}: {plan}");
    }
});

case!(setup_daw_takes_reapers_and_renoises_parts_only_from_the_runtimes_release_bundle, async {
    for daw in [DawId::Reaper, DawId::Renoise] {
        let lab = Lab::new(daw, false);
        let env = env_for(lab.root.path(), &lab.home, &lab.path("Renoise"));
        // A runtime with nothing laid out as a release lays it out: never this checkout's build.
        let bare = runtime(lab.root.path(), "plugin/state-2", &lab.home, daw, env, kumi_daw::io::Answers::default());
        match setup_on(&bare, true).await {
            Err(OpError::Failed { reason }) => assert!(reason.contains("doesn't carry"), "{reason}"),
            other => panic!("{daw:?} planned without a bundle: {other:?}"),
        }
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
case!(setup_daw_wont_replace_reapers_extension_while_reaper_has_it_loaded, async {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut std::ffi::c_void;
        fn FreeLibrary(module: *mut std::ffi::c_void) -> i32;
    }
    // Any DLL serves: REAPER's extension isn't built here. This one goes in, then REAPER loads it.
    let dll = std::fs::read(PathBuf::from(std::env::var("SystemRoot").unwrap()).join("System32").join("version.dll")).unwrap();
    let mut updated = dll.clone();
    updated.push(0);
    let lab = Lab::new(DawId::Reaper, false);
    carried(lab.root.path(), "loadable", None, &dll);
    let env = || env_for(lab.root.path(), &lab.home, &lab.path("Renoise"));
    let first = runtime(lab.root.path(), "plugin/state-2", &lab.home, DawId::Reaper, env(), carrying(&lab, "loadable"));
    setup_on(&first, false).await.unwrap();
    let extension = lab.path("REAPER/UserPlugins").join(daw_bridge::install::reaper_extension_name(daw_bridge::detect::Os::current()));
    // A runtime with other bytes at the same version: an update, while REAPER has the DLL loaded.
    carried(lab.root.path(), "update", None, &updated);
    let update = runtime(lab.root.path(), "plugin/state-3", &lab.home, DawId::Reaper, env(), carrying(&lab, "update"));
    let plan = setup_on(&update, true).await.unwrap();
    assert_eq!(plan["state"], "older", "{plan}");
    let before = lab.tree();
    let wide: Vec<u16> = extension.as_os_str().encode_wide().chain(Some(0)).collect();
    let held = unsafe { LoadLibraryW(wide.as_ptr()) };
    assert!(!held.is_null(), "{}", std::io::Error::last_os_error());
    let refused = call(&update, json!({"daw": "reaper", "dry_run": false, "expected": plan["digest"]})).await;
    assert_ne!(unsafe { FreeLibrary(held) }, 0);
    match refused {
        Err(OpError::Refused { code: "in_use", .. }) => {}
        other => panic!("replaced while loaded: {other:?}"),
    }
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
});

case!(setup_daw_leaves_a_copy_another_home_put_in_over_this_homes_even_with_this_homes_receipt, async {
    let lab = Lab::new(DawId::Renoise, false);
    lab.setup(false).await.unwrap();
    // Another Kumi (`kumi bridge` in a standalone home) has since put the same tool in, over this home's: its reference
    // names that home, while this home's receipt still says the tool is this home's.
    let tools = lab.path("Renoise/V3.5.4/Scripts/Tools");
    let target = daw_bridge::lifecycle::Target {
        daw: daw_bridge::Daw::Renoise,
        dir: tools.clone(),
        name: "com.kumi.Bridge.xrnx".into(),
        folder: true,
        reference: at(&tools, "com.kumi.Bridge.xrnx/bridge-reference.json"),
    };
    let other = lab.path("standalone/.kumi");
    daw_bridge::lifecycle::Lifecycle::new(&other, target.clone())
        .install(&lab.path("runtime/daws/renoise/com.kumi.Bridge.xrnx"), "1.0.0")
        .unwrap();
    let theirs = std::fs::read(&target.reference).unwrap();
    let plan = lab.setup(true).await.unwrap();
    assert_eq!((plan["state"].as_str(), &plan["paths"]), (Some("other_owner"), &json!([])), "{plan}");
    let before = lab.tree();
    assert_eq!(refused(lab.setup(false).await).0, "owned_elsewhere");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
    assert_eq!(std::fs::read(&target.reference).unwrap(), theirs, "it still names the other home");
    // A reference that names no home (a folder where the file was) says nothing of whose the copy is: it stays too.
    std::fs::remove_file(&target.reference).unwrap();
    std::fs::create_dir(&target.reference).unwrap();
    assert_eq!(lab.setup(true).await.unwrap()["state"], "other_owner");
});

case!(setup_daw_never_downgrades_reapers_extension_this_home_left_in_another_resource_folder, async {
    let lab = Lab::new(DawId::Reaper, false);
    // This home put in a newer extension (99.0.0) in this REAPER's resource folder...
    carried(lab.root.path(), "newer", Some("99.0.0"), b"the newer extension");
    let env = |resources: &str| {
        let mut env = env_for(lab.root.path(), &lab.home, &lab.path("Renoise"));
        env.insert("KUMI_REAPER_RESOURCE_DIR".into(), lab.path(resources).display().to_string());
        env
    };
    let newer = runtime(lab.root.path(), "plugin/state-2", &lab.home, DawId::Reaper, env("REAPER"), carrying(&lab, "newer"));
    setup_on(&newer, false).await.unwrap();
    // ...and a runtime carrying an older one now sets up another (a portable REAPER's), where there's none yet.
    std::fs::create_dir_all(lab.path("REAPER-2/UserPlugins")).unwrap();
    let older = runtime(lab.root.path(), "plugin/state-3", &lab.home, DawId::Reaper, env("REAPER-2"), carrying(&lab, "runtime"));
    let plan = setup_on(&older, true).await.unwrap();
    assert_eq!((plan["state"].as_str(), plan["version"].as_str(), &plan["paths"]), (Some("newer"), Some("99.0.0"), &json!([])), "{plan}");
    let before = lab.tree();
    assert_eq!(refused(setup_on(&older, false).await).0, "newer_installed");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
});

case!(setup_daw_takes_reapers_extensions_version_from_its_receipt_never_a_stale_file_beside_it, async {
    let lab = Lab::new(DawId::Reaper, false);
    // This home put in a newer extension (99.0.0); a version file an earlier install left beside it says 0.0.1.
    carried(lab.root.path(), "newer", Some("99.0.0"), b"the newer extension");
    let env = env_for(lab.root.path(), &lab.home, &lab.path("Renoise"));
    let newer = runtime(lab.root.path(), "plugin/state-2", &lab.home, DawId::Reaper, env, carrying(&lab, "newer"));
    setup_on(&newer, false).await.unwrap();
    std::fs::write(lab.path("REAPER/UserPlugins/reaper_kumi.version"), "0.0.1\n").unwrap();
    let plan = lab.setup(true).await.unwrap();
    assert_eq!((plan["state"].as_str(), plan["version"].as_str(), &plan["paths"]), (Some("newer"), Some("99.0.0"), &json!([])), "{plan}");
    let before = lab.tree();
    assert_eq!(refused(lab.install(&plan["digest"]).await).0, "newer_installed");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
});

case!(setup_daw_never_replaces_a_copy_whose_version_cant_be_ordered, async {
    let lab = Lab::new(DawId::Reaper, false);
    // This home put in a development build (no version file: recorded as "dev") in this REAPER's resource folder...
    carried(lab.root.path(), "dev", None, b"a development extension");
    std::fs::remove_file(lab.path("dev/daws/reaper/reaper_kumi.version")).unwrap();
    let env = |resources: &str| {
        let mut env = env_for(lab.root.path(), &lab.home, &lab.path("Renoise"));
        env.insert("KUMI_REAPER_RESOURCE_DIR".into(), lab.path(resources).display().to_string());
        env
    };
    let dev = runtime(lab.root.path(), "plugin/state-2", &lab.home, DawId::Reaper, env("REAPER"), carrying(&lab, "dev"));
    setup_on(&dev, false).await.unwrap();
    // ...and a released runtime now sets up a portable REAPER's, where there's none yet: the dev copy can't be ordered
    // against it, so it stays.
    std::fs::create_dir_all(lab.path("REAPER-2/UserPlugins")).unwrap();
    let released = runtime(lab.root.path(), "plugin/state-3", &lab.home, DawId::Reaper, env("REAPER-2"), carrying(&lab, "runtime"));
    let plan = setup_on(&released, true).await.unwrap();
    assert_eq!((plan["state"].as_str(), plan["version"].as_str(), &plan["paths"]), (Some("newer"), Some("dev"), &json!([])), "{plan}");
    assert!(plan["effects"].to_string().contains("can't be compared"), "{}", plan["effects"]);
    let before = lab.tree();
    assert_eq!(refused(setup_on(&released, false).await).0, "newer_installed");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
});

#[cfg(unix)]
case!(setup_daw_leaves_a_copy_whose_reference_it_cant_look_at_as_it_is, async {
    use std::os::unix::fs::PermissionsExt;
    let lab = Lab::new(DawId::Renoise, false);
    lab.setup(false).await.unwrap();
    // The tool's folder can't be opened any more, so its reference can't be looked at: this home's receipt still says
    // the tool is this home's, but nothing there says so now.
    let tool = lab.path("Renoise/V3.5.4/Scripts/Tools/com.kumi.Bridge.xrnx");
    let mode = |mode: u32| std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(mode)).unwrap();
    let before = lab.tree();
    mode(0o000);
    if std::fs::read_dir(&tool).is_ok() {
        // (root opens it anyway: nothing here to prove)
        return mode(0o755);
    }
    let plan = lab.setup(true).await.unwrap();
    let done = lab.install(&plan["digest"]).await;
    mode(0o755);
    let unknown = json!({"kumi_home": null});
    assert_eq!((plan["state"].as_str(), &plan["paths"], &plan["owner"]), (Some("other_owner"), &json!([]), &unknown), "{plan}");
    assert_eq!(refused(done).0, "owned_elsewhere");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
});
