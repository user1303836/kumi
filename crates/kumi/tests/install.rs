//! Original installed-update scenarios, using native bundle layout and executable startup probes.
use async_trait::async_trait;
use futures::FutureExt;
use kumi::{
    bridge_setup::{executable_name, run_program, Ran},
    config::json_files,
    install::*,
    tui::tty::TtyOutput,
};
use kumi_runtime::{
    ai::{
        error::LanguageModelError,
        http::{Fetch, FetchInit, Response},
    },
    core::{
        contracts::{MemoryScope, MemoryStore},
        store_backed::SqliteMemoryStore,
        store_client::StoreClient,
    },
    system::{self, Env, SystemProgram},
    KUMI_VERSION,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, fs, path::Path, rc::Rc};
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
    fn write(&self, text: &str) {
        self.0.borrow_mut().push_str(text);
    }
}
struct Serve {
    manifest: Value,
    bytes: Vec<u8>,
    status: u16,
    offline: bool,
    calls: RefCell<Vec<String>>,
}
impl Serve {
    fn new(manifest: Value) -> Rc<Self> {
        Rc::new(Self { manifest, bytes: vec![], status: 200, offline: false, calls: RefCell::new(vec![]) })
    }
}
#[async_trait(?Send)]
impl Fetch for Serve {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        assert!(init.signal.is_some());
        self.calls.borrow_mut().push(url.into());
        if self.offline {
            return Err(LanguageModelError::other("offline"));
        }
        if url.ends_with(".json") {
            return Ok(Response::json_response(self.status, self.manifest.clone()));
        }
        Ok(Response {
            body: Some(Box::pin(futures::stream::iter(vec![Ok(self.bytes.clone())]))),
            ..Response::text_response(self.status, "")
        })
    }
}
fn manifest(version: &str) -> Value {
    json!({"kumi":version,"bundle":"kumi.tar.gz","sha256":"a".repeat(64),"runtime":"rust-native","target":native_target()})
}
fn env(dir: &Path) -> Env {
    [
        ("KUMI_HOME".into(), dir.join("home").display().to_string()),
        ("KUMI_RELEASES".into(), "https://example.test/r".into()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), dir.join("Remote Scripts").display().to_string()),
        ("HOME".into(), dir.join("user").display().to_string()),
    ]
    .into()
}
fn io(env: &Env) -> (InstalledIo, Rc<Out>) {
    let out = Rc::new(Out::default());
    (InstalledIo::new(out.clone(), env.clone()), out)
}
fn put(path: impl AsRef<Path>, text: impl AsRef<[u8]>) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
#[tokio::test]
async fn versions_and_description_checked_before_use() {
    assert!(newer_version("1.1.0", "1.0.9"));
    assert!(newer_version("1.0.10", "1.0.9"));
    assert!(!newer_version("1.0.0", "1.0.0"));
    let good = manifest("1.2.3");
    let serve = Serve::new(good.clone());
    let env = [("KUMI_RELEASES".into(), "https://example.test/r/".into())].into();
    assert_eq!(serde_json::to_value(fetch_manifest(&env, Some(serve.clone())).await.unwrap()).unwrap(), good);
    assert_eq!(*serve.calls.borrow(), vec!["https://example.test/r/kumi-release.json"]);
    for (key, value) in [
        ("sha256", json!("short")),
        ("bundle", json!("../../evil.tar.gz")),
        ("kumi", json!("latest")),
        ("runtime", Value::Null),
        ("target", json!("../target")),
    ] {
        let mut bad = good.clone();
        bad[key] = value;
        assert!(fetch_manifest(&env, Some(Serve::new(bad))).await.is_none());
    }
    for serve in [
        Serve { offline: true, ..Rc::try_unwrap(Serve::new(good.clone())).ok().unwrap() },
        Serve { status: 404, ..Rc::try_unwrap(Serve::new(good)).ok().unwrap() },
    ] {
        assert!(fetch_manifest(&env, Some(Rc::new(serve))).await.is_none());
    }
}
#[tokio::test]
async fn release_check_distinguishes_newer_and_unreachable() {
    let env = Env::new();
    assert_eq!(check_release(&env, Some(Serve::new(manifest("99.0.0")))).await.unwrap().as_deref(), Some("99.0.0"));
    assert!(check_release(&env, Some(Serve::new(manifest(KUMI_VERSION)))).await.unwrap().is_none());
    let offline = Serve { offline: true, ..Rc::try_unwrap(Serve::new(Value::Null)).ok().unwrap() };
    assert!(check_release(&env, Some(Rc::new(offline))).await.unwrap_err().message().contains("couldn't reach GitHub"));
}
fn fake_release(dir: &Path, version: &str) -> Vec<u8> {
    let stage = dir.join(format!("stage-{version}"));
    fs::create_dir_all(&stage).unwrap();
    let code = dir.join("main.rs");
    put(&code, format!("fn main() {{ println!(\"Kumi {version}\"); }}"));
    let built = std::process::Command::new("rustc")
        .arg(&code)
        .arg("-o")
        .arg(stage.join(executable_name("kumi")))
        .arg("-C")
        .arg("debuginfo=0")
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    put(stage.join("package.json"), json!({"version":version}).to_string());
    let file = dir.join("kumi.tar.gz");
    assert!(std::process::Command::new(system::system_program_default(SystemProgram::Tar))
        .args(["-czf", file.to_str().unwrap(), "-C", stage.to_str().unwrap(), "."])
        .status()
        .unwrap()
        .success());
    fs::read(file).unwrap()
}
#[tokio::test]
async fn checked_executable_update_swap_and_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    put(home.join("app/package.json"), json!({"version":KUMI_VERSION}).to_string());
    put(home.join("app").join(executable_name("kumi")), "earlier");
    put(home.join("settings.json"), "{}");
    let bytes = fake_release(dir.path(), "99.0.0");
    let mut good = manifest("99.0.0");
    good["sha256"] = json!(hex::encode(Sha256::digest(&bytes)));
    let serve = |value: Value| -> Rc<dyn Fetch> {
        Rc::new(Serve { manifest: value, bytes: bytes.clone(), status: 200, offline: false, calls: RefCell::new(vec![]) })
    };
    let (mut tampered, out) = io(&env);
    let mut bad = good.clone();
    bad["sha256"] = json!("b".repeat(64));
    tampered.fetcher = Some(serve(bad));
    assert_eq!(update_installed(tampered).await.unwrap(), 1);
    assert!(out.0.borrow().contains("didn't match its checksum"));
    assert!(!home.join("app.previous").exists());
    let (mut other, out) = io(&env);
    let mut bad = good.clone();
    bad["target"] = json!("another-unknown-target");
    other.fetcher = Some(serve(bad));
    assert_eq!(update_installed(other).await.unwrap(), 1);
    assert!(out.0.borrow().contains("Run the installer again for this computer"));
    let (mut updated, out) = io(&env);
    updated.fetcher = Some(serve(good.clone()));
    let calls = Rc::new(RefCell::new(vec![]));
    let seen = calls.clone();
    updated.run = Some(Rc::new(move |command, args, cwd| {
        seen.borrow_mut().push((command.clone(), args.clone()));
        async move { run_program(&command, &args, cwd.as_deref()).await }.boxed_local()
    }));
    assert_eq!(update_installed(updated).await.unwrap(), 0, "{}", out.0.borrow());
    assert!(out.0.borrow().contains("Kumi is now 99.0.0"));
    assert!(calls.borrow().iter().any(|(cmd, _)| cmd == &system::system_program_default(SystemProgram::Tar)));
    assert!(calls.borrow().iter().any(|(cmd, args)| cmd.ends_with(&executable_name("kumi")) && args == &["--version"]));
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(home.join("app.previous/package.json")).unwrap()).unwrap()["version"],
        KUMI_VERSION
    );
    assert!(!home.join("app.new").exists());
    assert!(!home.join("downloads/kumi.tar.gz").exists());
    assert!(out.0.borrow().contains("To connect Live"));
    // What this Kumi kept in its database is written back for the older Kumi to read.
    let (database, _) = StoreClient::open(home.join("kumi.db"), json_files(&env).unwrap(), 1).await.unwrap();
    SqliteMemoryStore::new(database).remember(MemoryScope::Producer, None, "Mixes on headphones", None, 2).await.unwrap();
    let (rollback, out) = io(&env);
    assert_eq!(rollback_installed(rollback).await.unwrap(), 0);
    assert!(!out.0.borrow().contains("won't see"), "{}", out.0.borrow());
    assert_eq!(out.0.borrow().contains(&format!("Kumi windows still open keep running {KUMI_VERSION}")), !cfg!(windows));
    let written: Value = serde_json::from_slice(&fs::read(home.join("memory.json")).unwrap()).unwrap();
    assert_eq!(written["notes"][0]["text"], "Mixes on headphones");
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(home.join("app/package.json")).unwrap()).unwrap()["version"], KUMI_VERSION);
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(home.join("app.previous/package.json")).unwrap()).unwrap()["version"], "99.0.0");
    let (mut same, out) = io(&env);
    good["kumi"] = json!(KUMI_VERSION);
    same.fetcher = Some(serve(good));
    assert_eq!(update_installed(same).await.unwrap(), 0);
    assert!(out.0.borrow().contains("up to date"));
    // A write-back that can't run doesn't stop a rollback.
    for leftover in ["kumi.db", "kumi.db-wal", "kumi.db-shm"] {
        let _ = fs::remove_file(home.join(leftover));
    }
    fs::write(home.join("kumi.db"), "not a database").unwrap();
    let (rollback, out) = io(&env);
    assert_eq!(rollback_installed(rollback).await.unwrap(), 0);
    assert!(
        out.0.borrow().contains("The older Kumi won't see the notes, techniques or lessons kept since the update"),
        "{}",
        out.0.borrow()
    );
}
#[tokio::test]
async fn failed_unpack_or_probe_preserves_current_app() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    put(home.join("app/v"), "old");
    for (unpack, probe) in [(1, 0), (0, 1), (0, 0)] {
        let (mut io, out) = io(&env);
        let mut value = manifest("99.0.0");
        value["sha256"] = json!(hex::encode(Sha256::digest([])));
        io.fetcher = Some(Serve::new(value));
        io.run = Some(Rc::new(move |command, _, _| {
            async move {
                if command.ends_with(&executable_name("kumi")) {
                    Ran { code: probe, stdout: "wrong version".into(), stderr: "".into() }
                } else {
                    Ran { code: unpack, stdout: "".into(), stderr: "first\nlast\n".into() }
                }
            }
            .boxed_local()
        }));
        assert_eq!(update_installed(io).await.unwrap(), 1);
        assert_eq!(fs::read_to_string(home.join("app/v")).unwrap(), "old");
        assert!(!home.join("app.new").exists());
        assert!(!home.join("downloads/kumi.tar.gz").exists());
        assert!(out.0.borrow().contains(if unpack != 0 { "Unpacking it failed: last" } else { "new Kumi didn't start" }));
    }
}
#[tokio::test]
#[cfg(not(windows))]
async fn uninstall_preserves_producer_files_until_all_requested() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    for part in ["app", "app.previous", "node", "bin", "projects"] {
        fs::create_dir_all(home.join(part)).unwrap();
    }
    put(home.join("auth.json"), "{}");
    let (mut refused, _) = io(&env);
    refused.confirm = Some(Rc::new(|_| async { false }.boxed_local()));
    assert_eq!(uninstall_installed(refused, UninstallOptions::default()).await.unwrap(), 1);
    assert!(home.join("app").exists());
    assert_eq!(uninstall_installed(io(&env).0, UninstallOptions { all: false, yes: true }).await.unwrap(), 0);
    for part in ["app", "app.previous", "node", "bin"] {
        assert!(!home.join(part).exists());
    }
    assert!(home.join("auth.json").exists());
    assert!(home.join("projects").exists());
    assert_eq!(uninstall_installed(io(&env).0, UninstallOptions { all: true, yes: true }).await.unwrap(), 0);
    assert!(!home.exists());
}
fn with_bridge(dir: &Path) -> Env {
    let mut env = env(dir);
    let home = Path::new(&env["KUMI_HOME"]);
    for part in ["app", "node", "bin"] {
        fs::create_dir_all(home.join(part)).unwrap();
    }
    let package = home.join("bridge/1.0.52-1/package");
    put(package.join("package.json"), json!({"version":"1.0.52"}).to_string());
    put(package.join(executable_name("ableton-mcp-server")), "fixture");
    let config = home.join("bridge/state/bridge-config.json");
    put(
        &config,
        json!({"version":2,"server":{"command":package.join(executable_name("ableton-mcp-server")),"args":["--config",config]}})
            .to_string(),
    );
    put(Path::new(&env["KUMI_REMOTE_SCRIPTS_DIR"]).join("AbletonMcpBridge/bridge-reference.json"), json!({"config":config}).to_string());
    put(home.join("auth.json"), "{}");
    let extensions = dir.join("Ableton/Extensions");
    put(extensions.join("kumi.kumi/manifest.json"), "{}");
    fs::create_dir_all(dir.join("Ableton/Extensions Data/kumi.kumi")).unwrap();
    env.insert("KUMI_LIVE_EXTENSIONS_DIR".into(), extensions.display().to_string());
    env
}
#[tokio::test]
#[cfg(not(windows))]
async fn bridge_files_stay_while_live_uses_them() {
    for kind in ["declined", "live-open", "no-input", "uninstaller-refused", "removed"] {
        let dir = tempfile::tempdir().unwrap();
        let env = with_bridge(dir.path());
        let home = Path::new(&env["KUMI_HOME"]);
        let ext = Path::new(&env["KUMI_LIVE_EXTENSIONS_DIR"]);
        let (mut io, out) = io(&env);
        if kind != "no-input" {
            io.confirm = Some(Rc::new(move |_| async move { kind != "declined" }.boxed_local()));
        }
        io.live_running = Some(Rc::new(move || async move { kind == "live-open" }.boxed_local()));
        let calls = Rc::new(RefCell::new(vec![]));
        let seen = calls.clone();
        io.run = Some(Rc::new(move |command, args, _| {
            seen.borrow_mut().push((command, args));
            async move { Ran { code: if kind == "uninstaller-refused" { 1 } else { 0 }, stdout: "".into(), stderr: "".into() } }
                .boxed_local()
        }));
        assert_eq!(uninstall_installed(io, UninstallOptions { all: kind != "removed", yes: true }).await.unwrap(), 0);
        if kind == "removed" {
            assert!(calls
                .borrow()
                .iter()
                .any(|(cmd, args)| cmd.ends_with(&executable_name("ableton-mcp-server")) && args[0..2] == ["lifecycle", "uninstall"]));
            assert!(!ext.join("kumi.kumi").exists());
            assert!(!dir.path().join("Ableton/Extensions Data/kumi.kumi").exists());
            assert!(!home.join("bridge").exists());
            assert!(home.join("auth.json").exists());
            assert!(out.0.borrow().contains("bridge and Kumi's extension are out"));
        } else {
            assert!(home.join("bridge/state/bridge-config.json").exists());
            assert!(home.join("bridge/1.0.52-1").exists());
            assert!(!home.join("app").exists());
            assert!(!home.join("auth.json").exists());
            assert!(ext.join("kumi.kumi").exists());
            assert!(out.0.borrow().contains("while Live uses it."));
        }
    }
}
#[tokio::test]
#[cfg(not(windows))]
async fn path_removal_honors_zdotdir_and_preserves_unrelated_lines() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let user = Path::new(&env["HOME"]);
    let zdot = dir.path().join("zdot");
    fs::create_dir_all(home.join("app")).unwrap();
    put(zdot.join(".zshrc"), format!("alias ll='ls -l'\n\n{PATH_MARKER}\nexport PATH=\"{}/bin:$PATH\"\n", home.display()));
    put(user.join(".profile"), format!("{PATH_MARKER}\nexport EDITOR=vim\n"));
    put(user.join(".config/fish/conf.d/kumi.fish"), PATH_MARKER);
    let profile = user.join(".profile");
    let fish = user.join(".config/fish/conf.d/kumi.fish");
    env.insert("ZDOTDIR".into(), zdot.display().to_string());
    uninstall_installed(io(&env).0, UninstallOptions { all: false, yes: true }).await.unwrap();
    assert_eq!(fs::read_to_string(zdot.join(".zshrc")).unwrap(), "alias ll='ls -l'\n\n");
    assert_eq!(fs::read_to_string(profile).unwrap(), "export EDITOR=vim\n");
    assert!(!fish.exists());
}
#[tokio::test]
async fn no_release_is_distinct_from_offline_or_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let notfound = || Rc::new(Serve { status: 404, ..Rc::try_unwrap(Serve::new(Value::Null)).ok().unwrap() });
    assert_eq!(ask_release(&env, Some(notfound())).await, AskedRelease::None);
    assert_eq!(
        ask_release(&env, Some(Rc::new(Serve { offline: true, ..Rc::try_unwrap(Serve::new(Value::Null)).ok().unwrap() }))).await,
        AskedRelease::Offline
    );
    assert_eq!(ask_release(&env, Some(Serve::new(json!("<html>")))).await, AskedRelease::Invalid);
    assert!(check_release(&env, Some(notfound())).await.unwrap_err().message().contains("no Kumi release to get at example.test/r yet"));
    let (mut io, out) = io(&env);
    io.fetcher = Some(notfound());
    assert_eq!(update_installed(io).await.unwrap(), 1);
    assert!(out.0.borrow().contains("no Kumi release to get"));
    assert!(!out.0.borrow().contains("internet"));
}
#[tokio::test]
async fn failed_swap_restores_rollback_copy() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    let prev = dir.path().join("app.previous");
    put(app.join("v"), "2");
    put(prev.join("v"), "1");
    assert!(swap_in(dir.path().join("missing").to_str().unwrap(), app.to_str().unwrap(), prev.to_str().unwrap()).await.is_err());
    assert_eq!(fs::read_to_string(app.join("v")).unwrap(), "2");
    assert_eq!(fs::read_to_string(prev.join("v")).unwrap(), "1");
    let fresh = dir.path().join("app.new");
    put(fresh.join("v"), "3");
    swap_in(fresh.to_str().unwrap(), app.to_str().unwrap(), prev.to_str().unwrap()).await.unwrap();
    assert_eq!(fs::read_to_string(app.join("v")).unwrap(), "3");
    assert_eq!(fs::read_to_string(prev.join("v")).unwrap(), "2");
    assert!(!dir.path().join("app.previous.old").exists());
}
#[tokio::test]
async fn release_check_cache_is_daily_private_and_ignores_future_clock() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("latest.json");
    let cache = cache.to_str().unwrap();
    let serve = Serve::new(manifest("99.0.0"));
    assert_eq!(newer_release(cache, &Env::new(), Some(1000.), Some(serve.clone())).await.as_deref(), Some("99.0.0"));
    assert_eq!(serve.calls.borrow().len(), 1);
    assert_eq!(newer_release(cache, &Env::new(), Some(2000.), Some(serve.clone())).await.as_deref(), Some("99.0.0"));
    assert_eq!(serve.calls.borrow().len(), 1);
    newer_release(cache, &Env::new(), Some(999.), Some(serve.clone())).await;
    assert_eq!(serve.calls.borrow().len(), 2);
    newer_release(cache, &Env::new(), Some(999. + 86400000.), Some(serve.clone())).await;
    assert_eq!(serve.calls.borrow().len(), 3);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(cache).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[tokio::test]
async fn installed_update_migrates_equal_version_legacy_bridge_after_live_closes() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let config = home.join("bridge/state/bridge-config.json");
    put(home.join("app/package.json"), json!({"version":KUMI_VERSION,"bridge":"1.0.73"}).to_string());
    put(home.join("bridge/old/package/package.json"), json!({"version":"1.0.73"}).to_string());
    put(
        &config,
        json!({"server":{"command":"node","args":[home.join("bridge/old/package/dist/src/index.js"),"--config",config]}}).to_string(),
    );
    put(Path::new(&env["KUMI_REMOTE_SCRIPTS_DIR"]).join("AbletonMcpBridge/bridge-reference.json"), json!({"config":config}).to_string());
    for live in [true, false] {
        let (mut io, out) = io(&env);
        io.fetcher = Some(Serve::new(manifest(KUMI_VERSION)));
        io.live_running = Some(Rc::new(move || async move { live }.boxed_local()));
        let calls = Rc::new(RefCell::new(Vec::new()));
        io.update_bridge = Some(Rc::new({
            let calls = calls.clone();
            move |app| {
                calls.borrow_mut().push(app);
                async { 0 }.boxed_local()
            }
        }));
        assert_eq!(update_installed(io).await.unwrap(), 0);
        assert_eq!(calls.borrow().len(), usize::from(!live));
        assert!(out.0.borrow().contains("includes the native bridge (1.0.73)"));
    }
}

#[tokio::test]
async fn native_client_selects_target_from_legacy_compatible_release_index() {
    let selected = manifest("99.0.0");
    let mut index = json!({"kumi":"99.0.0","node":"24.0.0","bundle":"kumi.tar.gz","sha256":"b".repeat(64),"targets":{}});
    index["targets"][native_target()] = selected.clone();
    assert_eq!(serde_json::to_value(fetch_manifest(&Env::new(), Some(Serve::new(index.clone()))).await.unwrap()).unwrap(), selected);
    index["targets"] = json!({"another-platform": selected});
    assert_eq!(ask_release(&Env::new(), Some(Serve::new(index))).await, AskedRelease::Invalid);
}

#[tokio::test]
async fn launcher_handoff_skips_probes_and_rollback_restores_legacy_app_without_moving_user_data() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(dir.path());
    env.insert("KUMI_INSTALLED".into(), "1".into());
    let home = Path::new(&env["KUMI_HOME"]);
    let app = home.join("app");
    let previous = home.join("app.previous");
    let binary = app.join(executable_name("kumi"));
    put(&binary, "native runtime fixture");
    put(app.join("package.json"), json!({"version":KUMI_VERSION}).to_string());
    put(previous.join("package.json"), json!({"version":"1.7.3"}).to_string());
    put(previous.join("apps/kumi/bin/kumi.mjs"), "console.log('legacy')");
    put(home.join("node/retained-marker"), "for explicit rollback");
    let markers =
        ["auth.json", "settings.json", "history.json", "library/catalog.json", "memory/producer.json", "conversations/prior.json"];
    for marker in markers {
        put(home.join(marker), format!("preserve {marker}"));
    }
    let launcher_file = home.join("bin").join(if cfg!(windows) { "kumi.cmd" } else { "kumi" });
    put(&launcher_file, "old launcher");
    let fresh = home.join("app.new").join(executable_name("kumi"));
    put(&fresh, "probe fixture");
    assert!(!ensure_native_launcher(&env, &fresh).unwrap());
    assert_eq!(fs::read_to_string(&launcher_file).unwrap(), "old launcher");
    assert!(ensure_native_launcher(&env, &binary).unwrap());
    assert_eq!(fs::read_to_string(&launcher_file).unwrap(), launcher(cfg!(windows)));
    assert_eq!(rollback_installed(io(&env).0).await.unwrap(), 0);
    assert!(app.join("apps/kumi/bin/kumi.mjs").is_file());
    assert!(previous.join(executable_name("kumi")).is_file());
    assert!(home.join("node/retained-marker").exists());
    for marker in markers {
        assert_eq!(fs::read_to_string(home.join(marker)).unwrap(), format!("preserve {marker}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&launcher_file).unwrap().permissions().mode() & 0o777, 0o755);
    }
}

#[tokio::test]
async fn rollback_to_legacy_requires_closed_live_and_retained_legacy_bridge_generation() {
    for scenario in ["open", "missing", "refused", "ok"] {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path());
        let home = Path::new(&env["KUMI_HOME"]);
        let native = home.join("bridge/native/package");
        let old = home.join("bridge/legacy/package");
        let state = home.join("bridge/state");
        let config = state.join("bridge-config.json");
        let secret = state.join("bridge.secret");
        let scripts = Path::new(&env["KUMI_REMOTE_SCRIPTS_DIR"]);
        put(home.join("app").join(executable_name("kumi")), "native application");
        put(home.join("app/package.json"), json!({"version":KUMI_VERSION}).to_string());
        put(home.join("app.previous/apps/kumi/bin/kumi.mjs"), "legacy application");
        put(home.join("app.previous/package.json"), "{\"version\":\"1.7.3\"}");
        put(native.join("package.json"), "{\"version\":\"1.0.73\"}");
        put(native.join("release-manifest.json"), "{\"schema\":\"ableton-mcp-native-release/v1\"}");
        if scenario != "missing" {
            put(old.join("release-manifest.json"), "{\"schema\":\"ableton-mcp-release/v2\"}");
        }
        put(
            &config,
            json!({"server":{"command":native.join(executable_name("ableton-mcp-server")),"args":["--config",config]}}).to_string(),
        );
        put(scripts.join("AbletonMcpBridge/bridge-reference.json"), json!({"config":config}).to_string());
        let receipt = state.join("install-receipt.json");
        put(&receipt, json!({"version":1,"packageRoot":native,"stateDirectory":state,"configPath":config,"secretPath":secret,"remoteScriptsDirectory":scripts,"previous":{"packageRoot":old}}).to_string());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let (mut io, _) = io(&env);
        io.live_running = Some(Rc::new(move || async move { scenario == "open" }.boxed_local()));
        let calls = Rc::new(RefCell::new(Vec::new()));
        io.run = Some(Rc::new({
            let calls = calls.clone();
            move |command, args, _| {
                calls.borrow_mut().push((command, args));
                async move {
                    if scenario == "refused" {
                        Ran { code: 1, stdout: json!({"reason":"receipt drift"}).to_string(), stderr: String::new() }
                    } else {
                        Ran { code: 0, stdout: json!({"state":"completed"}).to_string(), stderr: String::new() }
                    }
                }
                .boxed_local()
            }
        }));
        let result = rollback_installed(io).await;
        if scenario == "ok" {
            assert_eq!(result.unwrap(), 0);
            assert!(home.join("app/apps/kumi/bin/kumi.mjs").is_file());
            let calls = calls.borrow();
            assert_eq!(calls.len(), 1);
            assert_eq!(&calls[0].1[..2], ["lifecycle", "rollback"]);
            assert!(calls[0].1.contains(&"--confirm-live-stopped".into()));
        } else {
            assert!(result.is_err());
            assert!(home.join("app").join(executable_name("kumi")).is_file());
            assert!(home.join("app.previous/apps/kumi/bin/kumi.mjs").is_file());
            assert_eq!(calls.borrow().len(), usize::from(scenario == "refused"));
        }
    }
}

/// A native app over a JavaScript bridge of the same version, as the old updater leaves it; `port` is
/// where the old Remote Script would answer.
fn legacy_bridge(dir: &Path, port: u16) -> (Env, String) {
    let env = env(dir);
    let home = dir.join("home");
    put(home.join("app/package.json"), json!({"version":"1.7.6","bridge":"1.0.74","runtime":"rust-native"}).to_string());
    put(home.join("app").join(executable_name("ableton-mcp-server")), "native bridge");
    let package = dir.join("old bridge/package");
    put(package.join("package.json"), json!({"version":"1.0.74"}).to_string());
    let config = dir.join("state/bridge-config.json");
    let entry = package.join("dist/src/cli.js");
    put(
        &config,
        json!({"version":2,"server":{"command":"/usr/bin/node","args":[entry, "--config", config]},"bridge":{"host":"127.0.0.1","port":port}})
            .to_string(),
    );
    put(dir.join("Remote Scripts/AbletonMcpBridge/bridge-reference.json"), json!({"config":config}).to_string());
    (env, fs::read_to_string(&config).unwrap())
}
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}
#[tokio::test]
async fn startup_switch_waits_quietly_while_live_has_the_old_remote_script() {
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (env, config) = legacy_bridge(dir.path(), listener.local_addr().unwrap().port());
    let (mut installed, out) = io(&env);
    installed.live_running =
        Some(Rc::new(|| async { panic!("Live's process list is not needed while the Remote Script answers") }.boxed_local()));
    installed.run = Some(Rc::new(|command, _, _| async move { panic!("nothing runs while Live is open: {command}") }.boxed_local()));
    finish_legacy_transition(&installed).await;
    assert_eq!(*out.0.borrow(), "");
    assert_eq!(fs::read_to_string(dir.path().join("state/bridge-config.json")).unwrap(), config);
}
#[tokio::test]
async fn startup_switch_failure_never_stops_kumi_and_leaves_no_partial_bridge() {
    // Unwritable: the bridge folder can't be made.
    let dir = tempfile::tempdir().unwrap();
    let (env, config) = legacy_bridge(dir.path(), free_port());
    put(dir.path().join("home/bridge"), "a file where the bridge folder goes");
    let (mut installed, out) = io(&env);
    installed.live_running = Some(Rc::new(|| async { false }.boxed_local()));
    finish_legacy_transition(&installed).await;
    assert!(out.0.borrow().contains("Kumi can still open; the bridge couldn't switch ("), "{}", out.0.borrow());
    assert_eq!(fs::read_to_string(dir.path().join("state/bridge-config.json")).unwrap(), config);
    // Stopped after its folder was made: the folder goes too.
    let dir = tempfile::tempdir().unwrap();
    let (env, config) = legacy_bridge(dir.path(), free_port());
    let (mut installed, out) = io(&env);
    installed.live_running = Some(Rc::new(|| async { false }.boxed_local()));
    installed.run =
        Some(Rc::new(|_, _, _| async { Ran { code: 1, stdout: String::new(), stderr: "no space left on device".into() } }.boxed_local()));
    finish_legacy_transition(&installed).await;
    assert!(
        out.0.borrow().contains("Kumi can still open. To finish switching the bridge, close Live and run: kumi bridge"),
        "{}",
        out.0.borrow()
    );
    let left: Vec<_> =
        fs::read_dir(dir.path().join("home/bridge")).map(|d| d.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(fs::read_to_string(dir.path().join("state/bridge-config.json")).unwrap(), config);
}
#[tokio::test]
async fn startup_switch_with_the_same_remote_script_is_quiet_and_never_moves_a_loaded_one() {
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (env, config) = legacy_bridge(dir.path(), listener.local_addr().unwrap().port());
    // The app's prepared bridge, whose Remote Script is byte for byte the one Live has loaded.
    let prepared = dir.path().join("home/app/bridge");
    let artifact = b"native bridge artifact";
    put(prepared.join("bridge.tar.gz"), artifact);
    put(prepared.join("prepared.json"), json!({"artifact":"bridge.tar.gz","sha256":hex::encode(Sha256::digest(artifact))}).to_string());
    put(prepared.join("package").join(executable_name("ableton-mcp-server")), "native bridge");
    for (name, text) in
        [("__init__.py", "from .ableton_mcp_remote_script import *\n"), ("ableton_mcp_remote_script.py", "BRIDGE = '1.0.74'\n")]
    {
        put(prepared.join("package/remote-script/AbletonMcpBridge").join(name), text);
        put(dir.path().join("Remote Scripts/AbletonMcpBridge").join(name), text);
    }
    put(dir.path().join("Remote Scripts/AbletonMcpBridge/manifest.json"), "{}");
    assert!(only_the_host_differs(&env), "the session has nothing to ask");
    // Live has it loaded (the port answers): its folder isn't touched, and nothing is said.
    let (mut open, out) = io(&env);
    open.live_running = Some(Rc::new(|| async { panic!("the port already says Live has it loaded") }.boxed_local()));
    open.run = Some(Rc::new(|command, _, _| async move { panic!("nothing runs while Live has it loaded: {command}") }.boxed_local()));
    finish_legacy_transition(&open).await;
    assert_eq!(*out.0.borrow(), "");
    assert_eq!(fs::read_to_string(dir.path().join("state/bridge-config.json")).unwrap(), config);
    // Live closed: the switch runs, quietly, and a failure is quiet too (a later start tries again).
    drop(listener);
    for code in [0, 1] {
        let calls = Rc::new(RefCell::new(Vec::<Vec<String>>::new()));
        let seen = calls.clone();
        let (mut closed, out) = io(&env);
        closed.live_running = Some(Rc::new(|| async { false }.boxed_local()));
        closed.run = Some(Rc::new(move |_, args, _| {
            let apply = args.iter().any(|a| a == "--apply");
            seen.borrow_mut().push(args);
            let answer = if code == 0 {
                json!({"state": if apply { "completed" } else { "planned" }})
            } else {
                json!({"version":"error","reason":"locked"})
            };
            async move { Ran { code, stdout: answer.to_string(), stderr: String::new() } }.boxed_local()
        }));
        finish_legacy_transition(&closed).await;
        assert_eq!(*out.0.borrow(), "", "code {code}");
        assert_eq!(calls.borrow()[0][..2].join(" "), "lifecycle upgrade");
        assert_eq!(calls.borrow().len(), if code == 0 { 2 } else { 1 });
    }
    // A different Remote Script: the session says how to switch again.
    put(dir.path().join("Remote Scripts/AbletonMcpBridge/ableton_mcp_remote_script.py"), "BRIDGE = '1.0.73'\n");
    assert!(!only_the_host_differs(&env));
}

/// A release whose bundle download starts and then never sends another byte.
struct Stalled(Value);
#[async_trait(?Send)]
impl Fetch for Stalled {
    async fn fetch(&self, url: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        if url.ends_with(".json") {
            return Ok(Response::json_response(200, self.0.clone()));
        }
        let first = futures::stream::iter(vec![Ok::<Vec<u8>, LanguageModelError>(vec![0u8; 1024])]);
        Ok(Response {
            body: Some(Box::pin(futures::StreamExt::chain(first, futures::stream::pending()))),
            ..Response::text_response(200, "")
        })
    }
}
#[tokio::test]
async fn ctrl_c_stops_a_stalled_update_download_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    put(home.join("app/package.json"), json!({"version":KUMI_VERSION}).to_string());
    put(home.join("app").join(executable_name("kumi")), "earlier");
    let (mut installed, out) = io(&env);
    installed.fetcher = Some(Rc::new(Stalled(manifest("99.0.0"))));
    let cancel = kumi_common::abort::Signal::new();
    installed.cancel = Some(cancel.clone());
    let pressed = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        cancel.cancel();
    });
    let updated =
        tokio::time::timeout(std::time::Duration::from_secs(10), update_installed(installed)).await.expect("the download stopped");
    pressed.await.unwrap();
    assert_eq!(updated.unwrap(), 1);
    assert!(out.0.borrow().contains("The download was stopped, so nothing was changed."), "{}", out.0.borrow());
    assert_eq!(fs::read_to_string(home.join("app").join(executable_name("kumi"))).unwrap(), "earlier");
    assert!(!home.join("app.new").exists() && !home.join("downloads/kumi.tar.gz").exists());
}
