//! `kumi update` for a checkout, rebuilding with Cargo.
use futures::FutureExt;
use kumi::{
    bridge_setup::{executable_name, Ran, Run},
    tui::tty::TtyOutput,
    update::*,
};
use kumi_runtime::system::Env;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
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
/// A checkout's bridge crate manifest, where a source update reads the bridge version it brings.
fn bridge_manifest(version: &str) -> String {
    format!("[package]\nname = \"ableton-mcp-server\"\nversion = \"{version}\"\nedition.workspace = true\n\n[dependencies]\nserde_json = {{ version = \"1\" }}\n")
}
struct Checkout {
    root: tempfile::TempDir,
    repo: String,
    env: Env,
}
impl Checkout {
    fn new(kumi: &str, bundled: &str, installed: Option<&str>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join("crates/ableton-mcp-server")).unwrap();
        fs::write(repo.join("package.json"), json!({"version":kumi}).to_string()).unwrap();
        fs::write(repo.join("crates/ableton-mcp-server/Cargo.toml"), bridge_manifest(bundled)).unwrap();
        let mut env = Env::from([("KUMI_REMOTE_SCRIPTS_DIR".into(), root.path().join("none").display().to_string())]);
        if let Some(installed) = installed {
            let scripts = root.path().join("Remote Scripts");
            let package = root.path().join("bridge/package");
            fs::create_dir_all(scripts.join("AbletonMcpBridge")).unwrap();
            fs::create_dir_all(&package).unwrap();
            fs::write(package.join("package.json"), json!({"version":installed}).to_string()).unwrap();
            let config = root.path().join("bridge-config.json");
            fs::write(
                &config,
                json!({"version":2,"server":{"command":package.join(executable_name("ableton-mcp-server")),"args":["--config",config]}})
                    .to_string(),
            )
            .unwrap();
            fs::write(scripts.join("AbletonMcpBridge/bridge-reference.json"), json!({"config":config}).to_string()).unwrap();
            env.insert("KUMI_REMOTE_SCRIPTS_DIR".into(), scripts.display().to_string());
        }
        Self { root, repo: repo.display().to_string(), env }
    }
    fn update(&self, out: Rc<Out>, run: Run) -> UpdateIo {
        let mut io = UpdateIo::new(out, self.env.clone());
        io.repo_dir = Some(self.repo.clone());
        io.run = Some(run);
        io
    }
    fn check(&self, run: Run) -> CheckIo {
        CheckIo {
            run: Some(run),
            repo_dir: Some(self.repo.clone()),
            version: Some("1.0.0".into()),
            cache_file: self.root.path().join("kumi/update-check.json").display().to_string(),
            ..Default::default()
        }
    }
}
#[derive(Default)]
struct Options {
    upstream: Option<String>,
    behind: u32,
    dirty: bool,
    offline: bool,
    on_merge: Option<Rc<dyn Fn()>>,
    failure: Option<String>,
}
fn programs(options: Options) -> (Run, Rc<RefCell<Vec<String>>>) {
    let calls = Rc::new(RefCell::new(vec![]));
    let run: Run = Rc::new({
        let calls = calls.clone();
        move |command, args, _| {
            let line = format!("{command} {}", args.join(" "));
            calls.borrow_mut().push(line.clone());
            let ok = |stdout: &str| Ran { code: 0, stdout: stdout.into(), stderr: "".into() };
            let ran = if options.failure.as_ref().is_some_and(|f| line.starts_with(f)) {
                Ran { code: 1, stdout: "".into(), stderr: "fixture failure".into() }
            } else if line.starts_with("git rev-parse") {
                ok("origin/main\n")
            } else if line.starts_with("git fetch") {
                if options.offline {
                    Ran { code: 128, stdout: "".into(), stderr: "fatal: unable to access".into() }
                } else {
                    ok("")
                }
            } else if line.starts_with("git show") {
                ok(&json!({"version":options.upstream.as_deref().unwrap_or("1.0.0")}).to_string())
            } else if line.starts_with("git status") {
                ok(if options.dirty { " M crates/kumi/src/cli.rs\n" } else { "" })
            } else if line.starts_with("git rev-list") {
                ok(&format!("{}\n", options.behind))
            } else if line.starts_with("git merge") {
                if let Some(on) = &options.on_merge {
                    on()
                }
                ok("")
            } else if line == "cargo build --release --locked --workspace --bins" {
                ok("")
            } else {
                Ran { code: 1, stdout: "".into(), stderr: format!("unexpected {line}") }
            };
            async move { ran }.boxed_local()
        }
    });
    (run, calls)
}
#[test]
fn versions_compare_by_number() {
    for (a, b, want) in [
        ("1.0.10", "1.0.9", true),
        ("1.1.0", "1.0.39", true),
        ("1.0.0", "1.0.0", false),
        ("0.9.9", "1.0.0", false),
        ("1.2.3-beta", "1.2.3", false),
        ("1.2.4oops", "1.2.3", true),
        ("1", "0.99.99", true),
    ] {
        assert_eq!(newer(a, b), want)
    }
}
#[tokio::test(flavor = "current_thread")]
async fn newer_checkout_is_found_at_most_once_a_day_and_silent_when_offline() {
    let c = Checkout::new("1.0.0", "1.0.39", None);
    let (run, calls) = programs(Options { upstream: Some("1.0.1".into()), ..Default::default() });
    let now = Rc::new(Cell::new(1000000.));
    let mut io = c.check(run);
    io.now = Some(Rc::new({
        let now = now.clone();
        move || now.get()
    }));
    assert_eq!(newer_kumi(io.clone()).await, Some("1.0.1".into()));
    assert!(calls.borrow().contains(&"git fetch --quiet origin main".into()));
    let asked = calls.borrow().len();
    now.set(now.get() + 3600000.);
    assert_eq!(newer_kumi(io.clone()).await, Some("1.0.1".into()));
    assert_eq!(calls.borrow().len(), asked);
    let mut updated = io.clone();
    updated.version = Some("1.0.1".into());
    assert_eq!(newer_kumi(updated).await, None);
    now.set(now.get() + 25. * 3600000.);
    newer_kumi(io).await;
    assert!(calls.borrow().len() > asked);
    let (offline, _) = programs(Options { offline: true, ..Default::default() });
    let mut io = c.check(offline);
    io.cache_file = c.root.path().join("other.json").display().to_string();
    assert_eq!(newer_kumi(io).await, None);
    fs::remove_dir_all(PathBuf::from(&c.repo).join(".git")).unwrap();
    let mut cached = c.check(programs(Options::default()).0);
    cached.now = Some(Rc::new({
        let now = now.clone();
        move || now.get()
    }));
    assert_eq!(newer_kumi(cached).await, Some("1.0.1".into()), "valid cache is used even outside checkout");
    let mut io = c.check(programs(Options::default()).0);
    io.cache_file = c.root.path().join("third.json").display().to_string();
    assert_eq!(newer_kumi(io).await, None);
}
#[tokio::test(flavor = "current_thread")]
async fn requested_check_reports_newest_or_explains_git_failure() {
    let c = Checkout::new("1.0.0", "1.0.0", None);
    assert_eq!(
        check_checkout(c.check(programs(Options { upstream: Some("1.1.0".into()), ..Default::default() }).0)).await.unwrap(),
        Some("1.1.0".into())
    );
    assert_eq!(check_checkout(c.check(programs(Options::default()).0)).await.unwrap(), None);
    assert!(check_checkout(c.check(programs(Options { offline: true, ..Default::default() }).0))
        .await
        .unwrap_err()
        .message()
        .contains("couldn't reach the repository"));
    fs::remove_dir_all(PathBuf::from(&c.repo).join(".git")).unwrap();
    assert!(check_checkout(c.check(programs(Options::default()).0)).await.unwrap_err().message().contains("isn't a git checkout"));
}
#[tokio::test(flavor = "current_thread")]
async fn update_moves_forward_rebuilds_native_and_updates_bridge_only_when_live_closed() {
    let c = Checkout::new("1.0.0", "1.0.39", Some("1.0.35"));
    let repo = c.repo.clone();
    let (run, calls) = programs(Options {
        behind: 3,
        on_merge: Some(Rc::new(move || {
            fs::write(PathBuf::from(&repo).join("package.json"), json!({"version":"1.0.1"}).to_string()).unwrap();
            fs::write(PathBuf::from(&repo).join("crates/ableton-mcp-server/Cargo.toml"), bridge_manifest("1.0.40")).unwrap();
        })),
        ..Default::default()
    });
    let out = Rc::new(Out::default());
    let bridged = Rc::new(RefCell::new(String::new()));
    let mut io = c.update(out.clone(), run);
    io.live_running = Some(Rc::new(|| async { false }.boxed_local()));
    io.update_bridge = Some(Rc::new({
        let bridged = bridged.clone();
        move |repo| {
            *bridged.borrow_mut() = repo;
            async { 0 }.boxed_local()
        }
    }));
    assert_eq!(run_update(io).await, 0);
    assert_eq!(
        calls.borrow().iter().filter(|line| line.contains("merge") || line.starts_with("cargo")).cloned().collect::<Vec<_>>(),
        ["git merge --ff-only origin/main", "cargo build --release --locked --workspace --bins"]
    );
    assert!(out.0.borrow().contains("Kumi is now 1.0.1."));
    assert_eq!(out.0.borrow().contains("Kumi windows opened before the update keep running"), !cfg!(windows));
    assert!(out.0.borrow().contains("The bridge in Live is 1.0.35; this Kumi's is 1.0.40."));
    assert_eq!(*bridged.borrow(), c.repo);
    let out = Rc::new(Out::default());
    let touched = Rc::new(Cell::new(false));
    let mut io = c.update(out.clone(), programs(Options::default()).0);
    io.live_running = Some(Rc::new(|| async { true }.boxed_local()));
    io.update_bridge = Some(Rc::new({
        let touched = touched.clone();
        move |_| {
            touched.set(true);
            async { 0 }.boxed_local()
        }
    }));
    assert_eq!(run_update(io).await, 0);
    assert!(out.0.borrow().contains("Kumi is up to date (1.0.1)."));
    assert!(out.0.borrow().contains("Quit Live (save your work first)"));
    assert!(!touched.get());
}
#[tokio::test(flavor = "current_thread")]
async fn dirty_checkout_is_preserved_and_current_or_missing_bridge_is_explained() {
    let c = Checkout::new("1.0.0", "1.0.39", Some("1.0.39"));
    let (run, calls) = programs(Options { dirty: true, behind: 2, ..Default::default() });
    let out = Rc::new(Out::default());
    assert_eq!(run_update(c.update(out.clone(), run)).await, 1);
    assert!(out.0.borrow().contains("changes of its own"));
    assert!(!calls.borrow().iter().any(|s| s.contains("merge") || s.starts_with("cargo")));
    let out = Rc::new(Out::default());
    assert_eq!(run_update(c.update(out.clone(), programs(Options::default()).0)).await, 0);
    assert!(out.0.borrow().contains("The bridge in Live is up to date."));
    let out = Rc::new(Out::default());
    let mut io = c.update(out.clone(), programs(Options::default()).0);
    io.env.insert("KUMI_REMOTE_SCRIPTS_DIR".into(), c.root.path().join("none").display().to_string());
    assert_eq!(run_update(io).await, 0);
    assert!(out.0.borrow().contains("To connect Live, quit Live"));
    for failure in ["git status", "git merge", "cargo build"] {
        let out = Rc::new(Out::default());
        assert_eq!(
            run_update(c.update(out.clone(), programs(Options { behind: 2, failure: Some(failure.into()), ..Default::default() }).0)).await,
            1
        );
        assert!(out.0.borrow().contains("fixture failure"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn same_version_legacy_bridge_migrates_but_newer_bridge_is_preserved() {
    let c = Checkout::new("1.0.0", "1.0.73", Some("1.0.73"));
    let config = c.root.path().join("bridge-config.json");
    fs::write(
        &config,
        json!({"server":{"command":"node","args":[c.root.path().join("bridge/package/dist/src/index.js"),"--config",config]}}).to_string(),
    )
    .unwrap();
    assert!(older_bridge(&c.env, Some("1.0.73")).unwrap().runtime_migration);
    assert!(older_bridge(&c.env, Some("1.0.72")).is_none());
    for live in [true, false] {
        let out = Rc::new(Out::default());
        let touched = Rc::new(Cell::new(false));
        let mut io = c.update(out.clone(), programs(Options::default()).0);
        io.live_running = Some(Rc::new(move || async move { live }.boxed_local()));
        io.update_bridge = Some(Rc::new({
            let touched = touched.clone();
            move |_| {
                touched.set(true);
                async { 0 }.boxed_local()
            }
        }));
        assert_eq!(run_update(io).await, 0);
        assert_eq!(touched.get(), !live);
        assert!(out.0.borrow().contains("includes the native bridge (1.0.73)"));
    }
}
