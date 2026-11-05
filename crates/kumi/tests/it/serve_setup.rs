//! `kumi serve`'s `setup_daw` (C8): Kumi's part in a DAW planned from files alone and put in exactly as planned. Live's
//! Remote Script goes in through a stand-in for the bridge's lifecycle (which writes what the real one leaves), Kumi
//! Ears for real; Renoise's tool through the shared lifecycle itself. Everything is in temporary folders: never the real
//! ~/.kumi or a DAW's.

use futures::FutureExt;
use kumi::serve::ctx::{Call, Ctx, Host as HostCall, OpError, Progress};
use kumi::serve::lease::Lease;
use kumi::serve::{registry, Args};
use kumi_common::abort::Signal;
use kumi_daw::process::{executable_name, Ran, Run};
use kumi_daw_ableton::app::answers::{LiveRunning, PreparedBridge};
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

/// A runtime's bundle of Live's bridge, as a release lays it out: `prepared.json` and the tarball it names, which holds
/// `package/` (the server and its package.json).
fn bundle(root: &Path) -> PathBuf {
    let prepared = root.join("runtime/bridge");
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
        let root = tempfile::tempdir().unwrap();
        let home = if adopted { root.path().join("standalone/.kumi") } else { root.path().join("plugin/kumi-home") };
        std::fs::create_dir_all(root.path().join("User Library/Remote Scripts")).unwrap();
        std::fs::create_dir_all(root.path().join("Renoise/V3.5.4/Scripts/Tools")).unwrap();
        if adopted {
            std::fs::create_dir_all(&home).unwrap();
        }
        let env = Env::from([
            ("HOME".into(), root.path().display().to_string()),
            ("KUMI_HOME".into(), home.display().to_string()),
            ("KUMI_NO_UPDATE_CHECK".into(), "1".into()),
            ("KUMI_PROCESSES".into(), "none".into()),
            ("KUMI_REMOTE_SCRIPTS_DIR".into(), root.path().join("User Library/Remote Scripts").display().to_string()),
            ("KUMI_RENOISE_PREFS_DIR".into(), root.path().join("Renoise").display().to_string()),
        ]);
        let calls = Rc::new(RefCell::new(vec![]));
        let live_open = Rc::new(Cell::new(false));
        let open = live_open.clone();
        let answers = kumi_daw::io::Answers::default()
            .with(PreparedBridge(bundle(root.path()).display().to_string()))
            .with(LiveRunning(Rc::new(move || {
                let open = open.get();
                async move { open }.boxed_local()
            })))
            .with(lifecycle(calls.clone()));
        let args = Args {
            state_dir: root.path().join("plugin/state"),
            kumi_home: home.clone(),
            daw,
            library_roots: vec![],
            media_roots: vec![],
            tools: BTreeMap::new(),
        };
        let ctx = Ctx::new(args, env, answers).unwrap();
        Lab { root, ctx, home, calls, live_open }
    }

    fn path(&self, path: &str) -> PathBuf {
        self.root.path().join(path)
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
}

/// The runtime's `setup_daw` for its own DAW.
async fn setup_on(ctx: &Ctx, dry_run: bool) -> Result<Value, OpError> {
    let operation = registry::OPERATIONS.iter().find(|operation| operation.name == "setup_daw").unwrap();
    let Value::Object(args) = json!({"daw": ctx.daw().as_str(), "dry_run": dry_run}) else { unreachable!() };
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
    let effects = plan["effects"].to_string();
    for path in plan["paths"].as_array().unwrap() {
        assert!(effects.contains(path.as_str().unwrap()), "an effect names {path}: {effects}");
    }
    assert!(effects.contains("Live must be closed"), "{effects}");
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
            assert_eq!(flag(call, "--state-dir"), lab.home.join("bridge/state").display().to_string());
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
        let config_dir = if earlier { lab.path(".config/ableton-mcp-release-a8a291e") } else { other.join("bridge/state") };
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
    installed_by(&lab.home, "99.0.0", &lab.scripts(), &lab.home.join("bridge/state"));
    let plan = lab.setup(true).await.unwrap();
    assert_eq!((plan["state"].as_str(), plan["version"].as_str(), &plan["paths"]), (Some("newer"), Some("99.0.0"), &json!([])));
    let before = lab.tree();
    assert_eq!(refused(lab.setup(false).await).0, "newer_installed");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &lab.lease());
});

case!(setup_daw_reports_an_adopted_installation_and_never_writes_into_it, async {
    let lab = Lab::new(DawId::Ableton, true);
    installed_by(&lab.home, &bundled(), &lab.scripts(), &lab.home.join("bridge/state"));
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
    assert_eq!(plan["paths"], strings(&[&tool, &lab.home.join("bridges/renoise")]), "{plan}");
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
    let mut env = lab.ctx.env().clone();
    env.insert("KUMI_HOME".into(), home.display().to_string());
    let args = Args {
        state_dir: lab.path("plugin/state-2"),
        kumi_home: home.clone(),
        daw: DawId::Renoise,
        library_roots: vec![],
        media_roots: vec![],
        tools: BTreeMap::new(),
    };
    let other = Ctx::new(args, env, kumi_daw::io::Answers::default()).unwrap();
    let plan = setup_on(&other, true).await.unwrap();
    assert_eq!((plan["state"].as_str(), &plan["paths"]), (Some("other_owner"), &json!([])), "{plan}");
    assert_eq!(plan["owner"], json!({"kumi_home": lab.home.display().to_string()}));
    let before = lab.tree();
    assert_eq!(refused(setup_on(&other, false).await).0, "owned_elsewhere");
    within(&differ(&before, &lab.tree()), &json!({"paths": []}), &home.join("serve/renoise.lease"));
});
