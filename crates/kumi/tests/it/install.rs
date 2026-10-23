//! Original installed-update scenarios, using native bundle layout and executable startup probes.
use async_trait::async_trait;
use futures::FutureExt;
use kumi::{config::json_files, install::*, tui::tty::TtyOutput};
use kumi_agent::core::{
    contracts::{MemoryScope, MemoryStore},
    store_backed::SqliteMemoryStore,
    store_client::StoreClient,
};
use kumi_ai::{
    error::LanguageModelError,
    http::{Fetch, FetchInit, Response},
};
use kumi_daw::process::{executable_name, run_program, Ran};
use kumi_daw::provider::newer as newer_version;
use kumi_daw_ableton::app::answers::{BridgeAfterUpdate, LiveRunning};
use kumi_daw_ableton::app::install::{finish_legacy_transition, only_the_host_differs};
use kumi_types::{
    system::{self, Env, SystemProgram},
    version::KUMI_VERSION,
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
        ("KUMI_HOME".into(), dir.join(".kumi").display().to_string()),
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
    fake_release_with(dir, version, None)
}
/// A release bundle whose `kumi --version` says `version`, carrying `bridge` when given.
fn fake_release_with(dir: &Path, version: &str, bridge: Option<&str>) -> Vec<u8> {
    fake_release_carrying(dir, version, bridge, &[])
}
/// The same, with more files in it (`daws/…`).
fn fake_release_carrying(dir: &Path, version: &str, bridge: Option<&str>, files: &[(&str, &str)]) -> Vec<u8> {
    let stage = dir.join(format!("stage-{version}"));
    fs::create_dir_all(&stage).unwrap();
    for (path, text) in files {
        put(stage.join(path), text);
    }
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
    put(stage.join("package.json"), json!({"version":version,"bridge":bridge}).to_string());
    put(
        stage.join("CHANGELOG.md"),
        format!("# Changelog\n\n## {version} — 2026-10-13\n\n- Kumi plays your Set back to you. It listens as it goes.\n- Faster starts.\n\n## {KUMI_VERSION} — 2026-10-12\n\n- This one.\n"),
    );
    let file = dir.join("kumi.tar.gz");
    assert!(std::process::Command::new(system::system_program_default(SystemProgram::Tar))
        .args(["-czf", file.to_str().unwrap(), "-C", stage.to_str().unwrap(), "."])
        .status()
        .unwrap()
        .success());
    fs::read(file).unwrap()
}
/// tar.exe reads its arguments in Windows' code page, which can't spell every folder name: an update unpacks in
/// KUMI_HOME, so the paths tar gets are plain ASCII however the folders above it are named.
#[tokio::test]
async fn an_update_unpacks_in_a_home_named_in_letters_a_code_page_lacks() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(&dir.path().join("John Smith ホーム é"));
    let home = Path::new(&env["KUMI_HOME"]);
    put(home.join("app/package.json"), json!({"version":KUMI_VERSION}).to_string());
    put(home.join("app").join(executable_name("kumi")), "earlier");
    let bytes = fake_release(dir.path(), "99.0.0");
    let mut good = manifest("99.0.0");
    good["sha256"] = json!(hex::encode(Sha256::digest(&bytes)));
    let (mut updated, out) = io(&env);
    updated.fetcher = Some(Rc::new(Serve { manifest: good, bytes, status: 200, offline: false, calls: RefCell::new(vec![]) }));
    let calls = Rc::new(RefCell::new(vec![]));
    let seen = calls.clone();
    updated.run = Some(Rc::new(move |command, args, cwd| {
        seen.borrow_mut().push((command.clone(), args.clone(), cwd.clone()));
        async move { run_program(&command, &args, cwd.as_deref()).await }.boxed_local()
    }));
    assert_eq!(update_installed(updated).await.unwrap(), 0, "{}", out.0.borrow());
    assert_eq!(version_of(home.join("app")), "99.0.0");
    let tar = system::system_program_default(SystemProgram::Tar);
    let (_, args, cwd) = calls.borrow().iter().find(|(command, _, _)| command == &tar).cloned().unwrap();
    assert_eq!(cwd.as_deref(), Some(home.to_str().unwrap()));
    assert!(args.iter().all(|arg| arg.is_ascii() && !Path::new(arg).is_absolute()), "{args:?}");
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
    updated.others = Some(Rc::new(|| async { 1 }.boxed_local()));
    let calls = Rc::new(RefCell::new(vec![]));
    let seen = calls.clone();
    updated.run = Some(Rc::new(move |command, args, cwd| {
        seen.borrow_mut().push((command.clone(), args.clone()));
        async move { run_program(&command, &args, cwd.as_deref()).await }.boxed_local()
    }));
    assert_eq!(update_installed(updated).await.unwrap(), 0, "{}", out.0.borrow());
    // What changed, from the new Kumi's own changelog, and the window still running this one.
    for line in [
        format!("Kumi {KUMI_VERSION} → 99.0.0. "),
        format!("update --rollback goes back to {KUMI_VERSION}."),
        "What's new:\n  Kumi plays your Set back to you.\n  Faster starts.\n  The whole notes: /changelog in Kumi\n".into(),
        format!("A Kumi window opened before the update keeps running {KUMI_VERSION} until you restart it."),
    ] {
        assert!(out.0.borrow().contains(&line), "{line}: {}", out.0.borrow());
    }
    assert!(calls.borrow().iter().any(|(cmd, _)| cmd == &system::system_program_default(SystemProgram::Tar)));
    assert!(calls.borrow().iter().any(|(cmd, args)| cmd.ends_with(&executable_name("kumi")) && args == &["--version"]));
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(home.join("app.previous/package.json")).unwrap()).unwrap()["version"],
        KUMI_VERSION
    );
    assert!(!home.join("app.new").exists());
    assert!(!home.join("downloads/kumi.tar.gz").exists());
    // No bridge in Live yet: the one way to connect it, said as everywhere Live runs.
    says_no_bridge_yet(&out.0.borrow());
    // What this Kumi kept in its database is written back for the older Kumi to read.
    let (database, _) = StoreClient::open(home.join("kumi.db"), json_files(&env).unwrap(), 1).await.unwrap();
    SqliteMemoryStore::new(database).remember(MemoryScope::Producer, None, "Mixes on headphones", None, 2).await.unwrap();
    let (mut rollback, out) = io(&env);
    rollback.others = Some(Rc::new(|| async { 2 }.boxed_local()));
    assert_eq!(rollback_installed(rollback).await.unwrap(), 0);
    assert!(!out.0.borrow().contains("won't see"), "{}", out.0.borrow());
    assert!(
        out.0.borrow().contains(&format!("2 Kumi windows still open keep running {KUMI_VERSION} until you close them;")),
        "{}",
        out.0.borrow()
    );
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
    // No other Kumi runs it: nothing said about windows.
    assert!(!out.0.borrow().contains("windows still open"), "{}", out.0.borrow());
}
#[cfg(unix)]
#[tokio::test]
async fn an_update_in_place_is_an_update_even_when_its_download_cant_be_removed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    put(home.join("app/package.json"), json!({"version":KUMI_VERSION}).to_string());
    put(home.join("app").join(executable_name("kumi")), "earlier");
    let bytes = fake_release(dir.path(), "99.0.0");
    let mut good = manifest("99.0.0");
    good["sha256"] = json!(hex::encode(Sha256::digest(&bytes)));
    // The download lands in a file that's there already, in a folder it can't be removed from afterwards (as a
    // file a scan holds can't be, on Windows).
    let downloads = home.join("downloads");
    put(downloads.join("kumi.tar.gz"), "");
    fs::set_permissions(&downloads, fs::Permissions::from_mode(0o555)).unwrap();
    let (mut updated, out) = io(&env);
    updated.fetcher = Some(Rc::new(Serve { manifest: good, bytes, status: 200, offline: false, calls: RefCell::new(vec![]) }));
    let code = update_installed(updated).await;
    fs::set_permissions(&downloads, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code.unwrap(), 0, "{}", out.0.borrow());
    assert!(out.0.borrow().contains(&format!("Kumi {KUMI_VERSION} → 99.0.0.")), "{}", out.0.borrow());
    // The update goes on to its bridge step, which has something to say where Live runs.
    says_no_bridge_yet(&out.0.borrow());
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
/// Kumi's folder as the installer leaves it: its launcher and its app, which an uninstall looks for before anything.
fn installed_kumi(home: &Path) {
    put(home.join("bin").join(if cfg!(windows) { "kumi.cmd" } else { "kumi" }), launcher(cfg!(windows)));
    put(home.join("app/package.json"), json!({"name":"kumi","version":KUMI_VERSION}).to_string());
    put(home.join("app").join(executable_name("kumi")), "kumi");
}
/// Every file under `folder`, with what it holds.
fn files_in(folder: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = vec![];
    for entry in fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_in(&path));
        } else {
            files.push((path.display().to_string(), fs::read(&path).unwrap()));
        }
    }
    files.sort();
    files
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
    installed_kumi(home);
    put(home.join("auth.json"), "{}");
    put(home.join("music").join("music.db"), "songs");
    let (mut refused, _) = io(&env);
    refused.confirm = Some(Rc::new(|_| async { false }.boxed_local()));
    assert_eq!(uninstall_installed(refused, UninstallOptions::default()).await.unwrap(), 1);
    assert!(home.join("app").exists());
    assert_eq!(uninstall_installed(io(&env).0, UninstallOptions { all: false, yes: true, ..Default::default() }).await.unwrap(), 0);
    for part in ["app", "app.previous", "node", "bin"] {
        assert!(!home.join(part).exists());
    }
    assert!(home.join("auth.json").exists());
    assert!(home.join("projects").exists());
    assert!(home.join("music").join("music.db").exists(), "what Kumi learned of their music is theirs until --all");
    // Without Kumi in it, the folder is the producer's to delete; installed again, --all takes everything.
    assert_eq!(uninstall_installed(io(&env).0, UninstallOptions { all: true, yes: true, ..Default::default() }).await.unwrap(), 1);
    assert!(home.join("auth.json").exists());
    installed_kumi(home);
    assert_eq!(uninstall_installed(io(&env).0, UninstallOptions { all: true, yes: true, ..Default::default() }).await.unwrap(), 0);
    assert!(!home.exists());
}
fn with_bridge(dir: &Path) -> Env {
    let mut env = env(dir);
    let home = Path::new(&env["KUMI_HOME"]);
    for part in ["app", "node", "bin"] {
        fs::create_dir_all(home.join(part)).unwrap();
    }
    installed_kumi(home);
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
        io.answers = io.answers.clone().with(LiveRunning(Rc::new(move || async move { kind == "live-open" }.boxed_local())));
        let calls = Rc::new(RefCell::new(vec![]));
        let seen = calls.clone();
        io.run = Some(Rc::new(move |command, args, _| {
            seen.borrow_mut().push((command, args));
            async move { Ran { code: if kind == "uninstaller-refused" { 1 } else { 0 }, stdout: "".into(), stderr: "".into() } }
                .boxed_local()
        }));
        assert_eq!(uninstall_installed(io, UninstallOptions { all: kind != "removed", yes: true, ..Default::default() }).await.unwrap(), 0);
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
/// Live's bridge is out but Kumi's Ears device won't come out of the User Library: that's said with where the device
/// is, and the rest goes on as for a bridge that's out (its files go too).
#[tokio::test]
#[cfg(unix)]
async fn an_ears_device_that_wont_come_out_is_said_with_where_it_is() {
    // SAFETY: geteuid has no preconditions. Root can delete from a read-only folder.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let env = with_bridge(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let library = dir.path().join("Kumi");
    put(library.join("Kumi Ears.amxd"), "device");
    fs::set_permissions(&library, fs::Permissions::from_mode(0o555)).unwrap();
    let (mut io, out) = io(&env);
    io.confirm = Some(Rc::new(|_| async { true }.boxed_local()));
    io.answers = io.answers.clone().with(LiveRunning(Rc::new(|| async { false }.boxed_local())));
    io.run = Some(Rc::new(|_, _, _| async { Ran { code: 0, stdout: "".into(), stderr: "".into() } }.boxed_local()));
    let code = uninstall_installed(io, UninstallOptions { yes: true, ..Default::default() }).await.unwrap();
    fs::set_permissions(&library, fs::Permissions::from_mode(0o755)).unwrap();
    let said = out.0.borrow().clone();
    assert_eq!(code, 0, "{said}");
    assert!(
        said.contains(&format!(
            "Kumi's Ears device couldn't be taken out of Live's User Library (Permission denied (os error 13)): delete {} by hand.",
            library.join("Kumi Ears.amxd").display()
        )),
        "{said}"
    );
    assert!(!said.contains("couldn't take its bridge out") && said.contains("The bridge and Kumi's extension are out of Live."), "{said}");
    assert!(!home.join("bridge").exists() && !home.join("app").exists(), "{said}");
}
#[tokio::test]
#[cfg(not(windows))]
async fn path_removal_honors_zdotdir_and_preserves_unrelated_lines() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let user = Path::new(&env["HOME"]);
    let zdot = dir.path().join("zdot");
    installed_kumi(home);
    put(zdot.join(".zshrc"), format!("alias ll='ls -l'\n\n{PATH_MARKER}\nexport PATH=\"{}/bin:$PATH\"\n", home.display()));
    put(user.join(".profile"), format!("{PATH_MARKER}\nexport EDITOR=vim\n"));
    put(user.join(".config/fish/conf.d/kumi.fish"), PATH_MARKER);
    let profile = user.join(".profile");
    let fish = user.join(".config/fish/conf.d/kumi.fish");
    env.insert("ZDOTDIR".into(), zdot.display().to_string());
    uninstall_installed(io(&env).0, UninstallOptions { all: false, yes: true, ..Default::default() }).await.unwrap();
    // The installer's blank line above its marker goes too.
    assert_eq!(fs::read_to_string(zdot.join(".zshrc")).unwrap(), "alias ll='ls -l'\n");
    assert_eq!(fs::read_to_string(profile).unwrap(), "export EDITOR=vim\n");
    assert!(!fish.exists());
}
#[tokio::test]
async fn no_release_is_distinct_from_offline_or_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let notfound = || Rc::new(Serve { status: 404, ..Rc::try_unwrap(Serve::new(Value::Null)).ok().unwrap() });
    assert_eq!(ask_release(&env, Some(notfound())).await, AskedRelease::None);
    assert!(matches!(
        ask_release(&env, Some(Rc::new(Serve { offline: true, ..Rc::try_unwrap(Serve::new(Value::Null)).ok().unwrap() }))).await,
        AskedRelease::Offline(..)
    ));
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
    let kumi = executable_name("kumi");
    put(app.join(&kumi), "2");
    put(prev.join(&kumi), "1");
    assert!(swap_in(dir.path().join("missing").to_str().unwrap(), app.to_str().unwrap(), prev.to_str().unwrap()).await.is_err());
    assert_eq!(fs::read_to_string(app.join(&kumi)).unwrap(), "2");
    assert_eq!(fs::read_to_string(prev.join(&kumi)).unwrap(), "1");
    let fresh = dir.path().join("app.new");
    put(fresh.join(&kumi), "3");
    swap_in(fresh.to_str().unwrap(), app.to_str().unwrap(), prev.to_str().unwrap()).await.unwrap();
    assert_eq!(fs::read_to_string(app.join(&kumi)).unwrap(), "3");
    assert_eq!(fs::read_to_string(prev.join(&kumi)).unwrap(), "2");
    assert!(!dir.path().join("app.previous.old").exists());
}
/// A Kumi window or an antivirus scan holds a file in Kumi's folder with no sharing, so Windows won't move the
/// folder: the update gives up with the Kumi that was there in place, and goes through once it's let go.
#[cfg(windows)]
#[tokio::test]
async fn a_kumi_folder_windows_holds_keeps_the_kumi_that_was_there() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (app, prev, fresh) = (dir.path().join("app"), dir.path().join("app.previous"), dir.path().join("app.new"));
    let kumi = executable_name("kumi");
    put(app.join(&kumi), "2");
    put(prev.join(&kumi), "1");
    put(fresh.join(&kumi), "3");
    let held = fs::OpenOptions::new().read(true).share_mode(0).open(app.join(&kumi)).unwrap();
    assert!(swap_in(fresh.to_str().unwrap(), app.to_str().unwrap(), prev.to_str().unwrap()).await.is_err());
    assert_eq!(fs::read_to_string(app.join(&kumi)).unwrap_err().raw_os_error(), Some(32), "still held, and still in app");
    drop(held);
    assert_eq!(fs::read_to_string(app.join(&kumi)).unwrap(), "2");
    assert_eq!(fs::read_to_string(prev.join(&kumi)).unwrap(), "1");
    swap_in(fresh.to_str().unwrap(), app.to_str().unwrap(), prev.to_str().unwrap()).await.unwrap();
    assert_eq!(fs::read_to_string(app.join(&kumi)).unwrap(), "3");
    assert_eq!(fs::read_to_string(prev.join(&kumi)).unwrap(), "2");
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
        io.answers = io.answers.clone().with(LiveRunning(Rc::new(move || async move { live }.boxed_local())));
        let calls = Rc::new(RefCell::new(Vec::new()));
        io.answers = io.answers.clone().with(BridgeAfterUpdate(Rc::new({
            let calls = calls.clone();
            move |app| {
                calls.borrow_mut().push(app);
                async { 0 }.boxed_local()
            }
        })));
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
        let mut env = env(dir.path());
        // Live's extensions: this version's copy is in Live; the retained bridge carries its own.
        let extensions = dir.path().join("Ableton/Extensions");
        env.insert("KUMI_LIVE_EXTENSIONS_DIR".into(), extensions.display().to_string());
        let home = Path::new(&env["KUMI_HOME"]).to_path_buf();
        let home = home.as_path();
        let native = home.join("bridge/native/package");
        let old = home.join("bridge/legacy/package");
        for (folder, version, code) in [
            (extensions.join("kumi.kumi"), "1.8.10", "// this version's"),
            (old.join("live-extension"), "1.0.0", "// the earlier bridge's"),
        ] {
            put(folder.join("manifest.json"), json!({"name":"kumi","version":version}).to_string());
            put(folder.join("dist/extension.js"), code);
        }
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
        io.answers = io.answers.clone().with(LiveRunning(Rc::new(move || async move { scenario == "open" }.boxed_local())));
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
            // Live's extension went back with the bridge: the earlier bridge checks it carries its own registry.
            assert_eq!(fs::read_to_string(extensions.join("kumi.kumi/dist/extension.js")).unwrap(), "// the earlier bridge's");
        } else {
            assert!(result.is_err());
            assert!(home.join("app").join(executable_name("kumi")).is_file());
            assert!(home.join("app.previous/apps/kumi/bin/kumi.mjs").is_file());
            assert_eq!(calls.borrow().len(), usize::from(scenario == "refused"));
            assert_eq!(fs::read_to_string(extensions.join("kumi.kumi/dist/extension.js")).unwrap(), "// this version's");
        }
    }
}

/// A native app over a JavaScript bridge of the same version, as the old updater leaves it; `port` is
/// where the old Remote Script would answer.
fn legacy_bridge(dir: &Path, port: u16) -> (Env, String) {
    let env = env(dir);
    let home = dir.join(".kumi");
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
    installed.answers = installed
        .answers
        .clone()
        .with(LiveRunning(Rc::new(|| async { panic!("Live's process list is not needed while the Remote Script answers") }.boxed_local())));
    installed.run = Some(Rc::new(|command, _, _| async move { panic!("nothing runs while Live is open: {command}") }.boxed_local()));
    finish_legacy_transition(&installed.daw()).await;
    assert_eq!(*out.0.borrow(), "");
    assert_eq!(fs::read_to_string(dir.path().join("state/bridge-config.json")).unwrap(), config);
}
#[tokio::test]
async fn startup_switch_failure_never_stops_kumi_and_leaves_no_partial_bridge() {
    // Unwritable: the bridge folder can't be made.
    let dir = tempfile::tempdir().unwrap();
    let (env, config) = legacy_bridge(dir.path(), free_port());
    put(dir.path().join(".kumi/bridge"), "a file where the bridge folder goes");
    let (mut installed, out) = io(&env);
    installed.answers = installed.answers.clone().with(LiveRunning(Rc::new(|| async { false }.boxed_local())));
    finish_legacy_transition(&installed.daw()).await;
    assert!(out.0.borrow().contains("Kumi can still open; the bridge couldn't switch ("), "{}", out.0.borrow());
    assert_eq!(fs::read_to_string(dir.path().join("state/bridge-config.json")).unwrap(), config);
    // Stopped after its folder was made: the folder goes too.
    let dir = tempfile::tempdir().unwrap();
    let (env, config) = legacy_bridge(dir.path(), free_port());
    let (mut installed, out) = io(&env);
    installed.answers = installed.answers.clone().with(LiveRunning(Rc::new(|| async { false }.boxed_local())));
    installed.run =
        Some(Rc::new(|_, _, _| async { Ran { code: 1, stdout: String::new(), stderr: "no space left on device".into() } }.boxed_local()));
    finish_legacy_transition(&installed.daw()).await;
    assert!(
        out.0.borrow().contains("Kumi can still open. To finish switching the bridge, close Live and run: kumi bridge"),
        "{}",
        out.0.borrow()
    );
    let left: Vec<_> =
        fs::read_dir(dir.path().join(".kumi/bridge")).map(|d| d.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(fs::read_to_string(dir.path().join("state/bridge-config.json")).unwrap(), config);
}
#[tokio::test]
async fn startup_switch_with_the_same_remote_script_is_quiet_and_never_moves_a_loaded_one() {
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (env, config) = legacy_bridge(dir.path(), listener.local_addr().unwrap().port());
    // The app's prepared bridge, whose Remote Script is byte for byte the one Live has loaded.
    let prepared = dir.path().join(".kumi/app/bridge");
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
    open.answers =
        open.answers.clone().with(LiveRunning(Rc::new(|| async { panic!("the port already says Live has it loaded") }.boxed_local())));
    open.run = Some(Rc::new(|command, _, _| async move { panic!("nothing runs while Live has it loaded: {command}") }.boxed_local()));
    finish_legacy_transition(&open.daw()).await;
    assert_eq!(*out.0.borrow(), "");
    assert_eq!(fs::read_to_string(dir.path().join("state/bridge-config.json")).unwrap(), config);
    // Live closed: the switch runs, quietly, and a failure is quiet too (a later start tries again).
    drop(listener);
    for code in [0, 1] {
        let calls = Rc::new(RefCell::new(Vec::<Vec<String>>::new()));
        let seen = calls.clone();
        let (mut closed, out) = io(&env);
        closed.answers = closed.answers.clone().with(LiveRunning(Rc::new(|| async { false }.boxed_local())));
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
        finish_legacy_transition(&closed.daw()).await;
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

/// A native Kumi going back to an earlier native one (1.8.11, whose bridge is 1.0.84), with the newer bridge 1.0.85
/// in Live and its extension. The bridge's receipt keeps `kept` (a bridge version) to go back to.
fn native_rollback(dir: &Path, kept: Option<&str>) -> (Env, std::path::PathBuf) {
    let mut env = env(dir);
    let extensions = dir.join("Ableton/Extensions");
    env.insert("KUMI_LIVE_EXTENSIONS_DIR".into(), extensions.display().to_string());
    let home = Path::new(&env["KUMI_HOME"]).to_path_buf();
    let native = json!({"schema":"ableton-mcp-native-release/v1"}).to_string();
    put(home.join("app").join(executable_name("kumi")), "the newer Kumi");
    put(home.join("app/package.json"), json!({"version":KUMI_VERSION,"bridge":"1.0.85"}).to_string());
    put(home.join("app.previous").join(executable_name("kumi")), "the earlier Kumi");
    put(home.join("app.previous/package.json"), json!({"version":"1.8.11","bridge":"1.0.84"}).to_string());
    put(home.join("app.previous/bridge/prepared.json"), json!({"artifact":"bridge.tar.gz","sha256":KEPT_ARTIFACT}).to_string());
    let extension = |folder: std::path::PathBuf, code: &str| {
        put(folder.join("manifest.json"), json!({"name":"kumi","version":"1.0.0"}).to_string());
        put(folder.join("dist/extension.js"), code);
    };
    let newer = home.join("bridge/1.0.85/package");
    put(newer.join("package.json"), json!({"version":"1.0.85"}).to_string());
    put(newer.join("release-manifest.json"), &native);
    extension(newer.join("live-extension"), "// 1.0.85's");
    extension(extensions.join("kumi.kumi"), "// 1.0.85's");
    let mut previous = Value::Null;
    if let Some(version) = kept {
        let root = home.join(format!("bridge/{version}/package"));
        put(root.join("package.json"), json!({"version":version}).to_string());
        put(root.join("release-manifest.json"), &native);
        extension(root.join("live-extension"), &format!("// {version}'s"));
        previous = json!({"packageRoot":root,"artifactSha256":KEPT_ARTIFACT});
    }
    let state = home.join("bridge/state");
    let config = state.join("bridge-config.json");
    let scripts = Path::new(&env["KUMI_REMOTE_SCRIPTS_DIR"]).to_path_buf();
    put(&config, json!({"server":{"command":newer.join(executable_name("ableton-mcp-server")),"args":["--config",config]}}).to_string());
    put(scripts.join("AbletonMcpBridge/bridge-reference.json"), json!({"config":config}).to_string());
    let receipt = state.join("install-receipt.json");
    put(&receipt, json!({"version":1,"packageRoot":newer,"stateDirectory":state,"configPath":config,"secretPath":state.join("bridge.secret"),"remoteScriptsDirectory":scripts,"previous":previous}).to_string());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600)).unwrap();
    }
    (env, home)
}
/// What a rollback did: its exit, what it said, the questions it asked and the bridge commands it ran. Live is open
/// at the looks `live` says (by count); `answers` are the producer's in a terminal, or None for none.
/// `on_run` sees each bridge command's count, and can answer it in the bridge's place.
struct RolledBack {
    code: i32,
    said: String,
    asked: Vec<String>,
    ran: Vec<Vec<String>>,
}
async fn roll_back(
    env: &Env,
    live: Rc<dyn Fn(usize) -> bool>,
    answers: Option<Vec<bool>>,
    ran: Rc<RefCell<Vec<Vec<String>>>>,
    on_run: Rc<dyn Fn(usize) -> Option<Ran>>,
) -> RolledBack {
    let (mut io, out) = io(env);
    let looks = Rc::new(std::cell::Cell::new(0));
    io.answers = io.answers.clone().with(LiveRunning(Rc::new(move || {
        looks.set(looks.get() + 1);
        let open = live(looks.get());
        async move { open }.boxed_local()
    })));
    let asked = Rc::new(RefCell::new(Vec::new()));
    if let Some(answers) = answers {
        let answers = Rc::new(RefCell::new(std::collections::VecDeque::from(answers)));
        let asked = asked.clone();
        io.confirm = Some(Rc::new(move |question| {
            asked.borrow_mut().push(question);
            let answer = answers.borrow_mut().pop_front().unwrap_or(false);
            async move { answer }.boxed_local()
        }));
    }
    let before = ran.borrow().len();
    io.run = Some(Rc::new({
        let ran = ran.clone();
        move |command, mut args, _| {
            assert!(command.ends_with(&executable_name("ableton-mcp-server")), "{command}");
            args.insert(0, command);
            ran.borrow_mut().push(args);
            let answer = on_run(ran.borrow().len()).unwrap_or_else(|| Ran {
                code: 0,
                stdout: json!({"state":"completed"}).to_string(),
                stderr: String::new(),
            });
            async move { answer }.boxed_local()
        }
    }));
    let code = rollback_installed(io).await.unwrap();
    let said = out.0.borrow().clone();
    let asked = asked.borrow().clone();
    let ran = ran.borrow()[before..].to_vec();
    RolledBack { code, said, asked, ran }
}
/// Live open for its first `n` looks, then closed.
fn open_for(n: usize) -> Rc<dyn Fn(usize) -> bool> {
    Rc::new(move |look| look <= n)
}
/// The SHA-256 the earlier Kumi's bundle lists for its bridge's artifact, which the receipt kept.
const KEPT_ARTIFACT: &str = "4b9a1d3f0c2e8a7b6d5c4b3a29180f7e6d5c4b3a29180f7e6d5c4b3a29180f7e";
fn version_of(app: impl AsRef<Path>) -> String {
    serde_json::from_slice::<Value>(&fs::read(app.as_ref().join("package.json")).unwrap()).unwrap()["version"].as_str().unwrap().into()
}
fn extension_in_live(dir: &Path) -> String {
    fs::read_to_string(dir.join("Ableton/Extensions/kumi.kumi/dist/extension.js")).unwrap()
}
fn calls() -> Rc<RefCell<Vec<Vec<String>>>> {
    Rc::new(RefCell::new(Vec::new()))
}
const STAYS: &str = "The bridge in Live stays 1.0.85, which works with 1.8.11.";
#[tokio::test]
async fn a_rollback_with_live_closed_puts_back_the_earlier_kumis_bridge_and_its_extension() {
    let dir = tempfile::tempdir().unwrap();
    let (env, home) = native_rollback(dir.path(), Some("1.0.84"));
    let done = roll_back(&env, open_for(0), None, calls(), Rc::new(|_| None)).await;
    assert_eq!(done.code, 0, "{}", done.said);
    assert!(done.said.contains("Kumi is back to 1.8.11."), "{}", done.said);
    assert!(done.said.contains("The bridge went back to 1.0.84 with it; Live loads it when it starts."), "{}", done.said);
    assert!(done.said.contains("Kumi's extension in Live went back with it."), "{}", done.said);
    assert!(done.said.contains("Putting back the bridge…"), "{}", done.said);
    assert!(!done.said.contains("up to date"), "{}", done.said);
    // The installed bridge's own rollback: it checks the kept generation against that one's registry.
    assert_eq!(done.ran.len(), 1);
    // By path components: on Windows the command's separators are backslashes.
    let installed = Path::new("1.0.85").join("package").join(executable_name("ableton-mcp-server"));
    assert!(Path::new(&done.ran[0][0]).ends_with(&installed), "{:?}", done.ran[0]);
    assert_eq!(&done.ran[0][1..3], ["lifecycle", "rollback"]);
    assert!(done.ran[0].contains(&"--confirm-live-stopped".into()));
    assert_eq!(version_of(home.join("app")), "1.8.11");
    assert_eq!(extension_in_live(dir.path()), "// 1.0.84's");
}
#[tokio::test]
async fn a_rollback_with_live_open_keeps_the_newer_bridge_unless_the_producer_closes_live_first() {
    // No terminal to ask in, a no, and a yes while Live stays open: the app goes back alone, and says how to finish.
    for (answers, open) in [(None, 1), (Some(vec![false]), 1), (Some(vec![true, true, true]), 9)] {
        let dir = tempfile::tempdir().unwrap();
        let (env, home) = native_rollback(dir.path(), Some("1.0.84"));
        let done = roll_back(&env, open_for(open), answers.clone(), calls(), Rc::new(|_| None)).await;
        assert_eq!(done.code, 0, "{}", done.said);
        assert!(done.said.contains(STAYS), "{}", done.said);
        assert!(done.said.contains("To put back bridge 1.0.84 too, quit Live, then run"), "{}", done.said);
        assert!(done.said.contains("update --rollback twice."), "{}", done.said);
        assert!(done.ran.is_empty());
        assert_eq!(version_of(home.join("app")), "1.8.11");
        assert_eq!(extension_in_live(dir.path()), "// 1.0.85's");
        if answers.is_some_and(|a| a.len() == 3) {
            assert_eq!(done.asked.len(), 3);
            assert!(
                done.asked[0].starts_with("Live is open. Going back to 1.8.11 can also put back its bridge 1.0.84"),
                "{:?}",
                done.asked
            );
            assert!(done.asked[1].starts_with("Live is still open."), "{:?}", done.asked);
        }
    }
    // A yes once Live is closed: the bridge goes back with the app.
    let dir = tempfile::tempdir().unwrap();
    let (env, home) = native_rollback(dir.path(), Some("1.0.84"));
    let done = roll_back(&env, open_for(1), Some(vec![true]), calls(), Rc::new(|_| None)).await;
    assert_eq!(done.code, 0, "{}", done.said);
    assert_eq!(done.asked.len(), 1);
    assert_eq!(done.ran.len(), 1);
    assert!(done.said.contains("The bridge went back to 1.0.84 with it"), "{}", done.said);
    assert_eq!(version_of(home.join("app")), "1.8.11");
    assert_eq!(extension_in_live(dir.path()), "// 1.0.84's");
}
#[tokio::test]
async fn a_failed_app_swap_changes_nothing_and_leaves_the_bridge_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (env, home) = native_rollback(dir.path(), Some("1.0.84"));
    let (mut io, out) = io(&env);
    // The earlier app goes missing just before the swap (here, as Kumi looks for Live), so the swap fails.
    let (previous, aside) = (home.join("app.previous"), home.join("app.previous.aside"));
    io.answers = io.answers.clone().with(LiveRunning(Rc::new({
        let (previous, aside) = (previous.clone(), aside.clone());
        move || {
            if previous.exists() {
                fs::rename(&previous, &aside).unwrap();
            }
            async { false }.boxed_local()
        }
    })));
    let ran = calls();
    io.run = Some(Rc::new({
        let ran = ran.clone();
        move |command, args, _| {
            ran.borrow_mut().push([vec![command], args].concat());
            async { Ran { code: 0, stdout: json!({"state":"completed"}).to_string(), stderr: String::new() } }.boxed_local()
        }
    }));
    assert_eq!(rollback_installed(io).await.unwrap(), 1);
    let said = out.0.borrow().clone();
    assert!(said.contains("Couldn't switch back"), "{said}");
    // The bridge goes back only after the app has: nothing ran, and the newer Kumi is where it was.
    assert!(ran.borrow().is_empty());
    assert_eq!(version_of(home.join("app")), KUMI_VERSION);
    assert!(!previous.exists());
    assert_eq!(version_of(&aside), "1.8.11");
    assert_eq!(extension_in_live(dir.path()), "// 1.0.85's");
}
#[tokio::test]
async fn a_rollback_without_the_earlier_kumis_bridge_kept_keeps_the_newer_one() {
    for kept in [None, Some("1.0.83")] {
        let dir = tempfile::tempdir().unwrap();
        let (env, home) = native_rollback(dir.path(), kept);
        let done = roll_back(&env, open_for(0), None, calls(), Rc::new(|_| None)).await;
        assert_eq!(done.code, 0, "{}", done.said);
        assert!(done.said.contains(STAYS), "{}", done.said);
        assert!(!done.said.contains("twice"), "{}", done.said);
        assert!(!done.said.contains("up to date"), "{}", done.said);
        assert!(done.ran.is_empty());
        assert_eq!(version_of(home.join("app")), "1.8.11");
        assert_eq!(extension_in_live(dir.path()), "// 1.0.85's");
    }
}
#[tokio::test]
async fn going_back_twice_after_an_app_only_rollback_puts_back_the_earlier_bridge_too() {
    let dir = tempfile::tempdir().unwrap();
    let (env, home) = native_rollback(dir.path(), Some("1.0.84"));
    let ran = calls();
    // Live open, no terminal: the app alone.
    let first = roll_back(&env, open_for(1), None, ran.clone(), Rc::new(|_| None)).await;
    assert!(first.said.contains("update --rollback twice."), "{}", first.said);
    assert_eq!(version_of(home.join("app")), "1.8.11");
    // Forward to the newer Kumi, whose bridge is the one in Live (here run by this Kumi, as any Kumi's is the same).
    let forward = roll_back(&env, open_for(0), None, ran.clone(), Rc::new(|_| None)).await;
    assert_eq!(forward.code, 0, "{}", forward.said);
    assert!(forward.said.contains("The bridge in Live is up to date."), "{}", forward.said);
    assert_eq!(version_of(home.join("app")), KUMI_VERSION);
    // Back again with Live closed: the bridge goes back with the app.
    let back = roll_back(&env, open_for(0), None, ran.clone(), Rc::new(|_| None)).await;
    assert_eq!(back.code, 0, "{}", back.said);
    assert!(back.said.contains("The bridge went back to 1.0.84 with it"), "{}", back.said);
    assert_eq!(ran.borrow().len(), 1);
    assert_eq!(version_of(home.join("app")), "1.8.11");
    assert_eq!(extension_in_live(dir.path()), "// 1.0.84's");
}
#[tokio::test]
async fn a_bridge_that_cant_go_back_stays_and_says_how_to_finish_or_where_to_look() {
    for (reason, code, says) in [
        (
            "no verified previous generation is available",
            0,
            "the one in Live stays, and works with 1.8.11. To try again, quit Live, then run",
        ),
        ("rollback failed and active-generation compensation was incomplete", 1, "doctor says how to repair it."),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (env, home) = native_rollback(dir.path(), Some("1.0.84"));
        let refused = json!({"version":"ableton-mcp-lifecycle-error/v1","reason":reason});
        let refuse: Rc<dyn Fn(usize) -> Option<Ran>> =
            Rc::new(move |_| Some(Ran { code: 1, stdout: String::new(), stderr: format!("{refused}\n") }));
        let done = roll_back(&env, open_for(0), None, calls(), refuse).await;
        assert_eq!(done.code, code, "{}", done.said);
        assert!(done.said.contains(reason), "{}", done.said);
        assert!(done.said.contains(says), "{}", done.said);
        assert_eq!(code == 1, done.said.contains("stopped partway"), "{}", done.said);
        assert_eq!(done.ran.len(), 1);
        assert_eq!(version_of(home.join("app")), "1.8.11");
        assert_eq!(extension_in_live(dir.path()), "// 1.0.85's");
    }
}
#[tokio::test]
async fn live_opening_while_the_app_swaps_leaves_the_bridge_and_says_how_to_finish() {
    let dir = tempfile::tempdir().unwrap();
    let (env, home) = native_rollback(dir.path(), Some("1.0.84"));
    // Closed at the first look, open at the one after the swap.
    let done = roll_back(&env, Rc::new(|look| look > 1), None, calls(), Rc::new(|_| None)).await;
    assert_eq!(done.code, 0, "{}", done.said);
    assert!(done.said.contains("Kumi is back to 1.8.11."), "{}", done.said);
    assert!(done.said.contains(STAYS), "{}", done.said);
    assert!(done.said.contains("update --rollback twice."), "{}", done.said);
    assert!(done.ran.is_empty());
    assert_eq!(version_of(home.join("app")), "1.8.11");
    assert_eq!(extension_in_live(dir.path()), "// 1.0.85's");
}
#[tokio::test]
async fn only_the_earlier_kumis_own_bridge_goes_back() {
    // The same version from another artifact (a checkout's, say) isn't that Kumi's.
    let dir = tempfile::tempdir().unwrap();
    let (env, home) = native_rollback(dir.path(), Some("1.0.84"));
    let receipt = home.join("bridge/state/install-receipt.json");
    let mut kept: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    kept["previous"]["artifactSha256"] = json!("0".repeat(64));
    fs::write(&receipt, kept.to_string()).unwrap();
    let done = roll_back(&env, open_for(0), None, calls(), Rc::new(|_| None)).await;
    assert!(done.said.contains(STAYS) && !done.said.contains("twice"), "{}", done.said);
    assert!(done.ran.is_empty());
    // A bridge run from the app's own folders, which the swap moves, stays too.
    let dir = tempfile::tempdir().unwrap();
    let (env, home) = native_rollback(dir.path(), Some("1.0.84"));
    let inside = home.join("app/bridge/package");
    put(inside.join("package.json"), json!({"version":"1.0.85"}).to_string());
    let state = home.join("bridge/state");
    let config = state.join("bridge-config.json");
    put(&config, json!({"server":{"command":inside.join(executable_name("ableton-mcp-server")),"args":["--config",config]}}).to_string());
    let receipt = state.join("install-receipt.json");
    let mut moved: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    moved["packageRoot"] = json!(inside);
    fs::write(&receipt, moved.to_string()).unwrap();
    let done = roll_back(&env, open_for(0), None, calls(), Rc::new(|_| None)).await;
    assert!(done.said.contains(STAYS) && !done.said.contains("twice"), "{}", done.said);
    assert!(done.ran.is_empty());
    assert_eq!(version_of(home.join("app")), "1.8.11");
}
#[test]
fn started_by_the_installer_kumi_says_how_to_start_it_next_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(dir.path());
    assert_eq!(next_time(&env), None);
    env.insert("KUMI_FROM_INSTALLER".into(), "1".into());
    assert_eq!(next_time(&env), None, "kumi is on this window's PATH");
    env.insert("KUMI_FROM_INSTALLER".into(), "new-window".into());
    assert_eq!(next_time(&env).as_deref(), Some("Next time: open a new terminal window and type kumi"));
    // With PATH left alone: the kumi command, ~ for the home folder, and quoted to type when it has a space.
    #[cfg(unix)]
    {
        env.insert("KUMI_FROM_INSTALLER".into(), "no-path".into());
        env.insert("KUMI_HOME".into(), dir.path().join("user/.kumi").display().to_string());
        assert_eq!(next_time(&env).as_deref(), Some("Next time: run ~/.kumi/bin/kumi, or add ~/.kumi/bin to PATH"));
        let spaced = dir.path().join("my kumi");
        env.insert("KUMI_HOME".into(), spaced.display().to_string());
        let (kumi, bin) = (spaced.join("bin/kumi"), spaced.join("bin"));
        assert_eq!(next_time(&env), Some(format!("Next time: run \"{}\", or add {} to PATH", kumi.display(), bin.display())));
    }
}
#[test]
fn an_updates_bridge_step_asks_nothing_more_and_waits_for_no_live() {
    // Live was just found closed: kumi bridge goes ahead (its lifecycle looks for Live again before it changes anything),
    // and doesn't wait ten minutes for a Live nobody has opened.
    let command = kumi_daw_ableton::app::install::bridge_command("/kumi/app", "/kumi");
    let command = command.as_std();
    assert_eq!(command.get_program(), std::ffi::OsStr::new(&kumi_library::sources::join("/kumi/app", &executable_name("kumi"))));
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["bridge", "--yes"]);
    let envs: std::collections::HashMap<String, Option<String>> = command
        .get_envs()
        .map(|(key, value)| (key.to_string_lossy().into_owned(), value.map(|value| value.to_string_lossy().into_owned())))
        .collect();
    for (key, value) in [("KUMI_BRIDGE_WAIT_SECONDS", "0"), ("KUMI_BRIDGE_AFTER", "1"), ("KUMI_INSTALLED", "1"), ("KUMI_HOME", "/kumi")] {
        assert_eq!(envs.get(key).cloned().flatten().as_deref(), Some(value), "{key}");
    }
}
/// Live's update ends with `kumi bridge` in a child Kumi: KUMI_DAW, which chooses the DAW sessions work with, is taken
/// out of its environment, so the child sets up Live's bridge whatever the producer exported (review L1).
#[test]
fn an_updates_bridge_step_is_lives_whatever_kumi_daw_says() {
    let command = kumi_daw_ableton::app::install::bridge_command("/kumi/app", "/kumi");
    let removed = command.as_std().get_envs().find(|(name, _)| *name == "KUMI_DAW").map(|(_, value)| value);
    assert_eq!(removed, Some(None), "KUMI_DAW taken out of the child's environment");
}
/// After an update, with no bridge in Live: how to connect it where Live runs, and on Linux nothing about Live at all.
fn says_no_bridge_yet(said: &str) {
    match kumi_daw_ableton::app::install::no_bridge_yet(system::platform()) {
        Some(words) => assert!(said.contains(&words), "{said}"),
        None => assert!(!said.contains("Live"), "nothing about Live where it doesn't run: {said}"),
    }
}
#[test]
fn with_no_bridge_in_live_an_update_says_how_to_connect_it_only_where_live_runs() {
    let words = format!("The bridge isn't in Live yet. {}.", *kumi_types::command::CONNECT_LIVE);
    for platform in ["darwin", "win32"] {
        assert_eq!(kumi_daw_ableton::app::install::no_bridge_yet(platform).as_deref(), Some(words.as_str()), "{platform}");
    }
    assert_eq!(kumi_daw_ableton::app::install::no_bridge_yet("linux"), None, "no Live for Linux");
}

/// Everything a producer keeps in Kumi's folder, which no uninstall without --all may touch.
const THEIRS: [&str; 17] = [
    "auth.json",
    "settings.json",
    "kumi.db",
    "kumi.db-wal",
    "memory.json",
    "techniques.json",
    "playbook.json",
    "gaps.jsonl",
    "timings.jsonl",
    "input-history",
    "whats-new.json",
    "audition-restore.json",
    "projects/0123456789abcdef/conversations/night-drive.json",
    "recipes/warm-pad.json",
    "library/state.json",
    "references/autechre.wav",
    "goals/mix.json",
];
/// Kumi's own: itself, what updates and the installer leave, and what it fetched.
fn kumi_s_own(home: &Path) {
    for (part, bytes) in [
        ("app/kumi", 4000),
        ("app.previous/kumi", 3000),
        ("app.previous.old/kumi", 10),
        ("app.rollback/kumi", 10),
        ("app.new/kumi", 10),
        ("app.replaced/kumi", 10),
        ("node/bin/node", 10),
        ("bin/kumi", 10),
        ("downloads/kumi.tar.gz", 10),
        ("tools/ffmpeg", 500_000),
        ("models/basic-pitch.onnx", 400_000),
        ("videos/youtube-dqw4w9wgxcq/frames/1.jpg", 90_000),
        (".install.Xy12Ab/kumi.tar.gz", 10),
    ] {
        put(home.join(part), vec![b'k'; bytes]);
    }
    put(home.join("app/package.json"), json!({"name":"kumi","version":KUMI_VERSION}).to_string());
}
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_dry_run_says_what_would_go_and_removes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let user = Path::new(&env["HOME"]);
    kumi_s_own(home);
    for file in THEIRS {
        put(home.join(file), "theirs");
    }
    let rc = format!("alias ll='ls -l'\n\n{PATH_MARKER}\nexport PATH=\"{}/bin:$PATH\"\n", home.display());
    put(user.join(".zshrc"), &rc);
    put(user.join(".config/fish/conf.d/kumi.fish"), PATH_MARKER);
    let (dry, out) = io(&env);
    assert_eq!(uninstall_installed(dry, UninstallOptions { dry_run: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    for line in [
        "kumi uninstall would remove from ",
        "(1 MB): app, app.previous, app.previous.old, app.new, app.replaced, app.rollback, node, bin, downloads, tools, models, videos, .install.Xy12Ab.",
        "It would also take Kumi out of ",
        ".zshrc and ",
        "kumi.fish.",
        "Your conversations, notes, recipes, sign-ins, library and references would stay in ",
        "Nothing was removed (--dry-run).",
    ] {
        assert!(said.contains(line), "{line}: {said}");
    }
    assert!(home.join("app/kumi").exists() && home.join("tools/ffmpeg").exists() && home.join(".install.Xy12Ab").exists());
    assert_eq!(fs::read_to_string(user.join(".zshrc")).unwrap(), rc);
    assert!(user.join(".config/fish/conf.d/kumi.fish").exists());
    let (all, out) = io(&env);
    assert_eq!(uninstall_installed(all, UninstallOptions { all: true, dry_run: true, ..Default::default() }).await.unwrap(), 0);
    assert!(out.0.borrow().contains("with your conversations, notes, recipes, sign-ins, library and references."), "{}", out.0.borrow());
    assert!(home.join("auth.json").exists());
}
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_removes_kumi_and_what_it_fetched_but_never_the_producers_files() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let user = Path::new(&env["HOME"]);
    kumi_s_own(home);
    for file in THEIRS {
        put(home.join(file), "theirs");
    }
    put(user.join(".config/fish/conf.d/kumi.fish"), PATH_MARKER);
    put(user.join(".profile"), format!("export EDITOR=vim\n\n{PATH_MARKER}\nexport PATH=\"{}/bin:$PATH\"\n", home.display()));
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap(), 0);
    for part in KUMI_S_OWN.iter().chain(&[".install.Xy12Ab"]) {
        assert!(!home.join(part).exists(), "{part} is still there");
    }
    for file in THEIRS {
        assert_eq!(fs::read_to_string(home.join(file)).unwrap(), "theirs", "{file}");
    }
    let said = out.0.borrow().clone();
    assert!(said.contains("Took Kumi out of fish (") && said.contains("kumi.fish is gone)."), "{said}");
    assert!(said.contains("Took Kumi out of ~/.profile.") || said.contains(".profile."), "{said}");
    assert!(
        said.contains("Kumi is removed, freeing 1 MB. Your conversations, notes, recipes, sign-ins, library and references are still in "),
        "{said}"
    );
    assert_eq!(fs::read_to_string(user.join(".profile")).unwrap(), "export EDITOR=vim\n");
    assert!(!user.join(".config/fish/conf.d/kumi.fish").exists());
}
#[tokio::test]
async fn update_version_installs_that_release_and_keeps_this_one_to_go_back_to() {
    let dir = tempfile::tempdir().unwrap();
    let env = with_bridge(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    put(home.join("app/package.json"), json!({"version":KUMI_VERSION}).to_string());
    put(home.join("app").join(executable_name("kumi")), "this one");
    // 1.15.0 brings bridge 1.0.50, older than the 1.0.52 in Live.
    let bytes = fake_release_with(dir.path(), "1.15.0", Some("1.0.50"));
    let mut older = manifest("1.15.0");
    older["sha256"] = json!(hex::encode(Sha256::digest(&bytes)));
    let serve = |value: Value, status: u16| -> Rc<Serve> {
        Rc::new(Serve { manifest: value, bytes: bytes.clone(), status, offline: false, calls: RefCell::new(vec![]) })
    };
    // Before going back, what the database keeps is written back for the older Kumi...
    let (database, _) = StoreClient::open(home.join("kumi.db"), json_files(&env).unwrap(), 1).await.unwrap();
    SqliteMemoryStore::new(database).remember(MemoryScope::Producer, None, "Sidechains with Ableton's Compressor", None, 2).await.unwrap();
    // ...but only once it has unpacked and started: one that doesn't start leaves the files as they were.
    let (mut broken, out) = io(&env);
    broken.version = Some("1.15.0".into());
    broken.fetcher = Some(serve(older.clone(), 200));
    broken.run = Some(Rc::new(|command, _, _| {
        async move { Ran { code: i32::from(command.ends_with(&executable_name("kumi"))), stdout: "".into(), stderr: "".into() } }
            .boxed_local()
    }));
    assert_eq!(update_installed(broken).await.unwrap(), 1);
    assert!(out.0.borrow().contains("The new Kumi didn't start, so this one stays."), "{}", out.0.borrow());
    assert!(!home.join("memory.json").exists(), "nothing written back for a Kumi that didn't go in");
    let (mut pinned, out) = io(&env);
    pinned.version = Some("1.15.0".into());
    pinned.fetcher = Some(serve(older.clone(), 200));
    assert_eq!(update_installed(pinned).await.unwrap(), 0, "{}", out.0.borrow());
    let said = out.0.borrow().clone();
    assert!(said.contains(&format!("Kumi {KUMI_VERSION} → 1.15.0, an earlier one. ")), "{said}");
    assert!(said.contains(&format!("update --rollback goes back to {KUMI_VERSION}.")), "{said}");
    assert!(said.contains("The bridge in Live stays 1.0.52, which works with 1.15.0."), "{said}");
    assert!(!said.contains("What's new"), "{said}");
    assert_eq!(version_of(home.join("app")), "1.15.0");
    assert_eq!(version_of(home.join("app.previous")), KUMI_VERSION);
    let written: Value = serde_json::from_slice(&fs::read(home.join("memory.json")).unwrap()).unwrap();
    assert_eq!(written["notes"][0]["text"], "Sidechains with Ableton's Compressor");
    // update --rollback returns to the Kumi that was there.
    let (mut rollback, out) = io(&env);
    rollback.answers = rollback.answers.clone().with(LiveRunning(Rc::new(|| async { false }.boxed_local())));
    rollback.answers = rollback.answers.clone().with(BridgeAfterUpdate(Rc::new(|_| async { 0 }.boxed_local())));
    rollback.others = Some(Rc::new(|| async { 1 }.boxed_local()));
    assert_eq!(rollback_installed(rollback).await.unwrap(), 0, "{}", out.0.borrow());
    assert!(
        out.0.borrow().contains(&format!("A Kumi window still open keeps running {KUMI_VERSION} until you close it;")),
        "{}",
        out.0.borrow()
    );
    assert_eq!(version_of(home.join("app")), KUMI_VERSION);
    // A version that isn't there, one another release answers for, one before native releases, and this one.
    for (version, answer, status, words) in [
        (
            "9.9.9",
            older.clone(),
            404,
            "There's no Kumi 9.9.9 to get (github.com/user1303836/kumi/releases lists every release). Nothing was changed.",
        ),
        ("1.14.0", older.clone(), 200, "There's no Kumi 1.14.0 at example.test; it has 1.15.0. Nothing was changed."),
        ("1.7.5", older.clone(), 200, "Kumi 1.7.5 came before Kumi's native releases, so kumi update can't install it"),
        ("1.14.0", json!({"nonsense":true}), 200, "Kumi 1.14.0's description didn't make sense; try again later. Nothing was changed."),
        (KUMI_VERSION, manifest(KUMI_VERSION), 200, "already."),
    ] {
        let (mut pinned, out) = io(&env);
        pinned.version = Some(version.into());
        let served = serve(answer, status);
        pinned.fetcher = Some(served.clone());
        pinned.answers = pinned.answers.clone().with(LiveRunning(Rc::new(|| async { true }.boxed_local())));
        let code = update_installed(pinned).await.unwrap();
        assert!(out.0.borrow().contains(words), "{version}: {}", out.0.borrow());
        assert_eq!(code, if version == KUMI_VERSION { 0 } else { 1 }, "{version}");
        let asked = version != "1.7.5" && version != KUMI_VERSION;
        assert_eq!(served.calls.borrow().len(), usize::from(asked), "{version}: only its description is asked for");
        assert_eq!(version_of(home.join("app")), KUMI_VERSION);
    }
    // This version is known without asking, offline too.
    let (mut current, out) = io(&env);
    current.version = Some(KUMI_VERSION.into());
    current.fetcher = Some(Rc::new(Serve { offline: true, ..Rc::try_unwrap(Serve::new(Value::Null)).ok().unwrap() }));
    current.answers = current.answers.clone().with(LiveRunning(Rc::new(|| async { true }.boxed_local())));
    assert_eq!(update_installed(current).await.unwrap(), 0, "{}", out.0.borrow());
    assert!(out.0.borrow().contains(&format!("Kumi is {KUMI_VERSION} already.")), "{}", out.0.borrow());
    // Pinned without KUMI_RELEASES: the release's own downloads on GitHub.
    assert_eq!(release_base_for(&Env::new(), Some("1.15.0")), "https://github.com/user1303836/kumi/releases/download/v1.15.0");
    assert_eq!(release_base_for(&env, Some("1.15.0")), "https://example.test/r");
    // An earlier Kumi whose bridge is newer than the one in Live brings it up to date, as any update does.
    let bytes = fake_release_with(dir.path(), "1.15.0", Some("1.0.60"));
    let mut newer_bridge = manifest("1.15.0");
    newer_bridge["sha256"] = json!(hex::encode(Sha256::digest(&bytes)));
    let (mut pinned, out) = io(&env);
    pinned.version = Some("1.15.0".into());
    pinned.fetcher = Some(Rc::new(Serve { manifest: newer_bridge, bytes, status: 200, offline: false, calls: RefCell::new(vec![]) }));
    pinned.answers = pinned.answers.clone().with(LiveRunning(Rc::new(|| async { true }.boxed_local())));
    assert_eq!(update_installed(pinned).await.unwrap(), 0, "{}", out.0.borrow());
    let said = out.0.borrow().clone();
    assert!(said.contains("The bridge in Live is 1.0.52; this Kumi's is 1.0.60.") && !said.contains("stays"), "{said}");
}
#[tokio::test]
async fn an_update_without_room_or_a_connection_changes_nothing_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    put(home.join("app/package.json"), json!({"version":KUMI_VERSION}).to_string());
    put(home.join("app").join(executable_name("kumi")), "earlier");
    let unchanged = || {
        assert_eq!(fs::read_to_string(home.join("app").join(executable_name("kumi"))).unwrap(), "earlier");
        assert!(!home.join("app.previous").exists() && !home.join("app.new").exists());
    };
    // Low on space: said before the download.
    let (mut low, out) = io(&env);
    let serve = Serve::new(manifest("99.0.0"));
    low.fetcher = Some(serve.clone());
    low.free = Some(Rc::new(|_| async { Some(120e6) }.boxed_local()));
    assert_eq!(update_installed(low).await.unwrap(), 1);
    assert!(out.0.borrow().contains("Kumi needs about 250 MB free to update, and the disk with "), "{}", out.0.borrow());
    assert!(out.0.borrow().contains("has 120 MB.") && out.0.borrow().ends_with("Nothing was changed.\n"), "{}", out.0.borrow());
    assert_eq!(serve.calls.borrow().len(), 1, "only the release's description was asked for");
    unchanged();
    // No connection, and a sign-in page answering instead of the release.
    let (mut offline, out) = io(&env);
    offline.fetcher = Some(Rc::new(Serve { offline: true, ..Rc::try_unwrap(Serve::new(Value::Null)).ok().unwrap() }));
    assert_eq!(update_installed(offline).await.unwrap(), 1);
    assert!(
        out.0.borrow().contains("Kumi couldn't reach example.test.") && out.0.borrow().contains("Nothing was changed."),
        "{}",
        out.0.borrow()
    );
    // The cause, from what the connection said: a name that didn't resolve.
    let (mut lost, out) = io(&env);
    lost.fetcher = Some(Rc::new(Unresolved));
    assert_eq!(update_installed(lost).await.unwrap(), 1);
    assert!(out.0.borrow().contains("Kumi couldn't reach example.test: its address couldn't be looked up (DNS)."), "{}", out.0.borrow());
    assert!(out.0.borrow().ends_with("Nothing was changed.\n"), "{}", out.0.borrow());
    // A proxy that isn't answering, told apart from one that turned Kumi away (web::cause's words).
    let (mut proxied, out) = io(&env);
    proxied.fetcher = Some(Rc::new(DeadProxy));
    assert_eq!(update_installed(proxied).await.unwrap(), 1);
    assert!(out.0.borrow().contains("Kumi couldn't reach example.test: the proxy isn't answering."), "{}", out.0.borrow());
    let (mut portal, out) = io(&env);
    portal.fetcher = Some(Rc::new(Page));
    assert_eq!(update_installed(portal).await.unwrap(), 1);
    assert!(out.0.borrow().contains("A Wi-Fi sign-in page or a proxy answered instead of example.test"), "{}", out.0.borrow());
    // The bundle refused, or cut off partway.
    for (status, broken, words) in [
        (403, false, "The download failed (it answered 403), so nothing was changed."),
        (407, false, "The download stopped: the proxy didn't let Kumi through, so nothing was changed."),
        (200, true, "The download stopped partway: the connection failed, so nothing was changed."),
    ] {
        let (mut cut, out) = io(&env);
        cut.fetcher = Some(Rc::new(Cut { manifest: manifest("99.0.0"), status, broken }));
        assert_eq!(update_installed(cut).await.unwrap(), 1);
        assert!(out.0.borrow().contains(words) && out.0.borrow().contains("nothing was changed"), "{}", out.0.borrow());
        unchanged();
    }
}
/// A connection whose name doesn't resolve.
struct Unresolved;
#[async_trait(?Send)]
impl Fetch for Unresolved {
    async fn fetch(&self, url: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        Err(LanguageModelError::other(format!(
            "error sending request for url ({url}): client error (Connect): dns error: failed to lookup address information: nodename nor servname provided, or not known"
        )))
    }
}
/// A proxy whose address refuses the connection.
struct DeadProxy;
#[async_trait(?Send)]
impl Fetch for DeadProxy {
    async fn fetch(&self, url: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        Err(LanguageModelError::other(format!(
            "error sending request for url ({url}): client error (Connect): tunnel error: proxy 127.0.0.1:9 connection refused"
        )))
    }
}
/// A Wi-Fi sign-in page, answering every address.
struct Page;
#[async_trait(?Send)]
impl Fetch for Page {
    async fn fetch(&self, _: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        Ok(Response::text_response(200, "<!DOCTYPE html><html><body>Sign in to Café Wi-Fi</body></html>"))
    }
}
/// The release's description, then a bundle refused with `status` or cut off partway.
struct Cut {
    manifest: Value,
    status: u16,
    broken: bool,
}
#[async_trait(?Send)]
impl Fetch for Cut {
    async fn fetch(&self, url: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        if url.ends_with(".json") {
            return Ok(Response::json_response(200, self.manifest.clone()));
        }
        let chunks: Vec<Result<Vec<u8>, LanguageModelError>> =
            if self.broken { vec![Ok(vec![1; 64]), Err(LanguageModelError::other("connection reset by peer"))] } else { vec![] };
        Ok(Response { body: Some(Box::pin(futures::stream::iter(chunks))), ..Response::text_response(self.status, "") })
    }
}
#[cfg(unix)]
#[tokio::test]
async fn an_update_in_a_folder_another_account_owns_says_whose_before_anything_moves() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    // /usr stands in for a ~/.kumi an install run with sudo left to root.
    let (mut theirs, out) = io(&Env::from([("KUMI_HOME".into(), "/usr".into())]));
    let serve = Serve::new(manifest("99.0.0"));
    theirs.fetcher = Some(serve.clone());
    assert_eq!(update_installed(theirs).await.unwrap(), 1);
    assert_eq!(*out.0.borrow(), "/usr isn't yours (installed with sudo?): sudo chown -R $USER /usr Nothing was changed.\n");
    assert!(serve.calls.borrow().is_empty());
}
/// GitHub right after a release: its API names `newest` (or answers `api` instead), that release has its own
/// description when `described`, and `releases/latest` still leads to the one before (98.0.0).
struct Hub {
    api: u16,
    newest: &'static str,
    described: bool,
    bundle: Vec<u8>,
    sha256: String,
    calls: RefCell<Vec<String>>,
}
impl Hub {
    fn new(api: u16, described: bool) -> Rc<Self> {
        Rc::new(Self { api, newest: "99.0.0", described, bundle: vec![], sha256: "a".repeat(64), calls: RefCell::new(vec![]) })
    }
}
#[async_trait(?Send)]
impl Fetch for Hub {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        assert!(!init.headers.keys().any(|key| key.eq_ignore_ascii_case("authorization")), "no token is ever sent");
        self.calls.borrow_mut().push(url.into());
        let own = format!("https://github.com/user1303836/kumi/releases/download/v{}", self.newest);
        if url == "https://api.github.com/repos/user1303836/kumi/releases/latest" {
            assert!(init.headers.contains_key("user-agent"), "GitHub's API wants a user agent");
            return Ok(Response::json_response(self.api, json!({"tag_name":format!("v{}", self.newest)})));
        }
        if url == format!("{own}/kumi-release.json") {
            let mut newest = manifest(self.newest);
            newest["sha256"] = json!(self.sha256);
            return Ok(if self.described { Response::json_response(200, newest) } else { Response::text_response(404, "Not Found") });
        }
        if url == format!("{own}/kumi.tar.gz") {
            return Ok(Response {
                body: Some(Box::pin(futures::stream::iter(vec![Ok(self.bundle.clone())]))),
                ..Response::text_response(200, "")
            });
        }
        assert_eq!(url, "https://github.com/user1303836/kumi/releases/latest/download/kumi-release.json");
        Ok(Response::json_response(200, manifest("98.0.0")))
    }
}
#[tokio::test]
async fn an_asked_for_update_takes_the_release_githubs_api_names_and_the_usual_way_without_it() {
    let env = Env::new();
    // Right after a release: the API names it while releases/latest still leads to the one before.
    let fresh = Hub::new(200, true);
    assert_eq!(check_release(&env, Some(fresh.clone())).await.unwrap().as_deref(), Some("99.0.0"));
    assert_eq!(fresh.calls.borrow().len(), 2);
    // The API's rate limit, or a release without its description yet: the usual way, without a word about it.
    for (api, described) in [(403, true), (429, true), (200, false)] {
        assert_eq!(check_release(&env, Some(Hub::new(api, described))).await.unwrap().as_deref(), Some("98.0.0"), "{api} {described}");
    }
    // KUMI_RELEASES is used as it is.
    let set = Env::from([("KUMI_RELEASES".into(), "https://example.test/r".into())]);
    let serve = Serve::new(manifest("97.0.0"));
    assert_eq!(check_release(&set, Some(serve.clone())).await.unwrap().as_deref(), Some("97.0.0"));
    assert_eq!(*serve.calls.borrow(), ["https://example.test/r/kumi-release.json"]);
    // The daily check asks once, the usual way.
    let daily = Hub::new(200, true);
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("update-check.json");
    assert_eq!(newer_release(cache.to_str().unwrap(), &env, Some(1000.), Some(daily.clone())).await.as_deref(), Some("98.0.0"));
    assert_eq!(*daily.calls.borrow(), ["https://github.com/user1303836/kumi/releases/latest/download/kumi-release.json"]);
    // kumi update downloads from the release the API named, checked against that release's own checksum.
    let home = dir.path().join("home");
    put(home.join("app/package.json"), json!({"version":KUMI_VERSION}).to_string());
    put(home.join("app").join(executable_name("kumi")), "earlier");
    let bundle = fake_release(dir.path(), "99.0.0");
    let hub = Rc::new(Hub { sha256: hex::encode(Sha256::digest(&bundle)), bundle, ..Rc::try_unwrap(Hub::new(200, true)).ok().unwrap() });
    let installed: Env = [
        ("KUMI_HOME".into(), home.display().to_string()),
        ("HOME".into(), dir.path().join("user").display().to_string()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), dir.path().join("Remote Scripts").display().to_string()),
    ]
    .into();
    let (mut updating, out) = io(&installed);
    updating.fetcher = Some(hub.clone());
    assert_eq!(update_installed(updating).await.unwrap(), 0, "{}", out.0.borrow());
    assert!(out.0.borrow().contains(&format!("Kumi {KUMI_VERSION} → 99.0.0.")), "{}", out.0.borrow());
    assert_eq!(hub.calls.borrow().last().unwrap(), "https://github.com/user1303836/kumi/releases/download/v99.0.0/kumi.tar.gz");
}
/// Kumi's PATH line as the installer writes it, or edited to start from $HOME, ${HOME} or ~: each is Kumi's under its
/// marker, as install.sh counts it. Another Kumi's line, and Kumi's folder named outside a marker, stay.
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_takes_out_kumis_path_line_however_it_is_spelled_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("user");
    let home = user.join(".kumi");
    let mut env = env(dir.path());
    env.insert("KUMI_HOME".into(), home.display().to_string());
    installed_kumi(&home);
    let rc = |name: &str, kumi: &str| (name.to_string(), format!("alias ll='ls -l'\n\n{PATH_MARKER}\n{kumi}\n"));
    let ours = [
        rc(".zshrc", &format!("export PATH=\"{}/bin:$PATH\"", home.display())),
        rc(".zprofile", "export PATH=\"$HOME/.kumi/bin:$PATH\""),
        rc(".bashrc", "export PATH=\"${HOME}/.kumi/bin:$PATH\""),
        rc(".bash_profile", "export PATH=~/.kumi/bin:$PATH"),
    ];
    for (name, text) in &ours {
        put(user.join(name), text);
    }
    // Another Kumi's, under its own marker, and this Kumi's folder named by hand under none.
    let theirs = format!("{PATH_MARKER}\nexport PATH=\"/opt/other-kumi/bin:$PATH\"\nexport PATH=\"$HOME/.kumi/bin:$PATH\"\n");
    put(user.join(".profile"), &theirs);
    let other_fish = format!("{PATH_MARKER}\ncontains -- \"/opt/other-kumi/bin\" $PATH; or set -gx PATH \"/opt/other-kumi/bin\" $PATH\n");
    let fish = user.join(".config/fish/conf.d/kumi.fish");
    put(&fish, format!("\n{PATH_MARKER}\ncontains -- \"~/.kumi/bin\" $PATH; or set -gx PATH \"~/.kumi/bin\" $PATH\n{other_fish}"));
    let (dry, out) = io(&env);
    assert_eq!(uninstall_installed(dry, UninstallOptions { dry_run: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    for (name, _) in &ours {
        assert!(said.contains(&format!("/{name}")), "{name}: {said}");
    }
    assert!(said.contains("kumi.fish") && !said.contains(".profile"), "{said}");
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap(), 0);
    for (name, _) in &ours {
        assert_eq!(fs::read_to_string(user.join(name)).unwrap(), "alias ll='ls -l'\n", "{name}: {}", out.0.borrow());
    }
    assert_eq!(fs::read_to_string(user.join(".profile")).unwrap(), theirs);
    // The other Kumi's fish lines stay, so the file does.
    assert_eq!(fs::read_to_string(&fish).unwrap(), other_fish);
    assert!(out.0.borrow().contains("Took Kumi out of ~/.config/fish/conf.d/kumi.fish."), "{}", out.0.borrow());
}
/// A folder that isn't Kumi's own keeps everything in it, asked or not: the home folder (a scratch one, as HOME and
/// KUMI_HOME both), a disk's root and where a disk is mounted (dry runs only), a relative path, a folder that isn't
/// there, and one without Kumi's launcher and app.
#[tokio::test]
async fn uninstall_refuses_a_folder_that_isnt_kumis_own() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("user");
    // Kumi installed into the home folder itself, among the producer's own folders and startup file.
    installed_kumi(&user);
    for theirs in ["Downloads/song.wav", "Videos/clip.mov", "models/mine.onnx", "tools/mixdown.sh", "bin/backup.sh"] {
        put(user.join(theirs), "theirs");
    }
    put(user.join(".profile"), format!("alias ll='ls -l'\n\n{PATH_MARKER}\nexport PATH=\"{}/bin:$PATH\"\n", user.display()));
    let before = files_in(&user);
    let mut env = env(dir.path());
    env.insert("HOME".into(), user.display().to_string());
    env.insert("KUMI_HOME".into(), user.display().to_string());
    assert_eq!(not_kumis_own(&env["KUMI_HOME"], &env), Some(NotKumis::Home));
    for options in [
        UninstallOptions { dry_run: true, ..Default::default() },
        UninstallOptions { yes: true, ..Default::default() },
        UninstallOptions { all: true, yes: true, ..Default::default() },
    ] {
        let (io, out) = io(&env);
        assert_eq!(uninstall_installed(io, options).await.unwrap(), 1);
        let said = out.0.borrow().clone();
        assert!(
            said.starts_with("Kumi won't uninstall from ~: it's your home folder, so the names Kumi uses there (bin, downloads, tools…) could be your own. Nothing was removed."),
            "{said}"
        );
        #[cfg(not(windows))]
        assert!(
            said.contains(&format!(
                "Kumi's own there, to take out by hand: ~/app and ~/bin/kumi, and the two lines under \"{PATH_MARKER}\""
            )),
            "{said}"
        );
        assert_eq!(files_in(&user), before);
    }
    // A disk's root, and where a disk is mounted: dry runs only.
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    assert_eq!(not_kumis_own(root, &env), Some(NotKumis::Root));
    #[cfg(unix)]
    assert_eq!(not_kumis_own("/dev", &env), Some(NotKumis::Root));
    let mut at_root = env.clone();
    at_root.insert("KUMI_HOME".into(), root.into());
    for all in [false, true] {
        let (io, out) = io(&at_root);
        assert_eq!(uninstall_installed(io, UninstallOptions { all, dry_run: true, ..Default::default() }).await.unwrap(), 1);
        let said = out.0.borrow().clone();
        assert!(
            said.starts_with(&format!("Kumi won't uninstall from {root}: it's the root of a disk")) && !said.contains("would remove"),
            "{said}"
        );
    }
    // A relative path, a folder that isn't there, and one without Kumi's launcher and app.
    let elsewhere = dir.path().join("elsewhere");
    for theirs in ["downloads/song.wav", "tools/mixdown.sh", "bin/backup.sh", "app/notes.txt"] {
        put(elsewhere.join(theirs), "theirs");
    }
    let unmarked = files_in(&elsewhere);
    for (folder, why, words) in [
        ("relkumi".to_string(), NotKumis::Relative, "Kumi won't uninstall from \"relkumi\": KUMI_HOME is a relative path"),
        (
            "~".to_string(),
            NotKumis::Relative,
            "Kumi won't uninstall from \"~\": KUMI_HOME starts with ~, which only a shell turns into your home folder.",
        ),
        ("~/.kumi".to_string(), NotKumis::Relative, "KUMI_HOME starts with ~, which only a shell turns into your home folder."),
        (
            dir.path().join("nowhere").display().to_string(),
            NotKumis::Missing,
            "(Kumi's folder) to uninstall Kumi from. Nothing was removed.",
        ),
        (elsewhere.display().to_string(), NotKumis::Unmarked, "it holds no Kumi launcher (bin/kumi) and app"),
    ] {
        assert_eq!(not_kumis_own(&folder, &env), Some(why), "{folder}");
        let mut there = env.clone();
        there.insert("KUMI_HOME".into(), folder.clone());
        let (io, out) = io(&there);
        assert_eq!(uninstall_installed(io, UninstallOptions { all: true, yes: true, ..Default::default() }).await.unwrap(), 1);
        assert!(out.0.borrow().contains(words), "{folder}: {}", out.0.borrow());
    }
    assert_eq!(files_in(&elsewhere), unmarked);
    assert!(!dir.path().join("nowhere").exists());
}
/// Kumi's parts are taken by their exact names, so a Downloads beside them stays; from a fetched folder (tools, models,
/// videos) only what Kumi put there goes, and the folder too once nothing else is left in it.
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_takes_kumis_names_by_their_case_and_leaves_folders_holding_more() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    installed_kumi(home);
    for (file, text) in [
        ("Downloads/song.wav", "theirs"),
        ("videos/youtube-dqw4w9wgxcq/meta.json", "{}"),
        ("videos/holiday.mov", "theirs"),
        ("tools/ffmpeg/ffmpeg", "Kumi's"),
        ("tools/mixdown.sh", "theirs"),
        ("models/basic-pitch.onnx", "Kumi's"),
        ("models/slots/0123456789ab/model.onnx", "Kumi's"),
    ] {
        put(home.join(file), text);
    }
    let (dry, out) = io(&env);
    assert_eq!(uninstall_installed(dry, UninstallOptions { dry_run: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    assert!(
        said.contains("): app, bin, tools/ffmpeg, models, videos/youtube-dqw4w9wgxcq.")
            && said.contains("/tools and ")
            && said.contains("/videos stay: they hold more than Kumi put there."),
        "{said}"
    );
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap(), 0, "{}", out.0.borrow());
    for kept in ["Downloads/song.wav", "videos/holiday.mov", "tools/mixdown.sh"] {
        assert!(home.join(kept).exists(), "{kept}");
    }
    for gone in ["videos/youtube-dqw4w9wgxcq", "tools/ffmpeg", "models", "app", "bin"] {
        assert!(!home.join(gone).exists(), "{gone}");
    }
}
/// A part that can't go doesn't leave Kumi half removed with a bare error: every other part is tried, what stayed is
/// said with why, and Kumi itself (app, bin and its PATH line) stays so kumi uninstall can run again.
#[cfg(unix)]
#[tokio::test]
async fn uninstall_tries_every_part_says_what_stayed_and_keeps_kumi_to_run_again() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions. Root removes anything.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let user = Path::new(&env["HOME"]);
    kumi_s_own(home);
    for file in THEIRS {
        put(home.join(file), "theirs");
    }
    let rc = format!("{PATH_MARKER}\nexport PATH=\"{}/bin:$PATH\"\n", home.display());
    put(user.join(".profile"), &rc);
    // A folder in app.previous this account can't empty.
    let stuck = home.join("app.previous/locked");
    put(stuck.join("file"), "x");
    fs::set_permissions(&stuck, fs::Permissions::from_mode(0o555)).unwrap();
    let (removing, out) = io(&env);
    let code = uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap();
    fs::set_permissions(&stuck, fs::Permissions::from_mode(0o755)).unwrap();
    let said = out.0.borrow().clone();
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("Kumi is partly removed: ")
            && said.contains("/app.previous couldn't go (Permission denied (os error 13)). Kumi itself stays, so you can run kumi uninstall again once that's sorted (if another account owns it: sudo chown -R $USER "),
        "{said}"
    );
    assert!(!said.contains("Took Kumi out of"), "{said}");
    for part in
        ["app.previous.old", "app.new", "app.replaced", "app.rollback", "node", "downloads", "tools", "models", "videos", ".install.Xy12Ab"]
    {
        assert!(!home.join(part).exists(), "{part} is still there");
    }
    assert!(home.join("app/kumi").exists() && home.join("bin/kumi").exists());
    assert_eq!(fs::read_to_string(user.join(".profile")).unwrap(), rc);
    for file in THEIRS {
        assert_eq!(fs::read_to_string(home.join(file)).unwrap(), "theirs", "{file}");
    }
    // Sorted, it runs again and finishes, the PATH line last.
    let (again, out) = io(&env);
    assert_eq!(uninstall_installed(again, UninstallOptions { yes: true, ..Default::default() }).await.unwrap(), 0, "{}", out.0.borrow());
    assert!(!home.join("app").exists() && !home.join("app.previous").exists() && !home.join("bin").exists());
    assert_eq!(fs::read_to_string(user.join(".profile")).unwrap(), "");
}
/// A startup file uninstall can't change is named with the lines to take out by hand, in the dry run too.
#[cfg(unix)]
#[tokio::test]
async fn uninstall_names_a_startup_file_it_cant_change_and_the_lines_to_take_out() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions. Root writes anything.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let user = Path::new(&env["HOME"]);
    installed_kumi(home);
    let line = format!("export PATH=\"{}/bin:$PATH\"", home.display());
    let rc = format!("alias ll='ls -l'\n\n{PATH_MARKER}\n{line}\n");
    let profile = user.join(".profile");
    put(&profile, &rc);
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o444)).unwrap();
    let lines = format!("lines 3–4: {PATH_MARKER} / {line}");
    let (dry, out) = io(&env);
    assert_eq!(uninstall_installed(dry, UninstallOptions { dry_run: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    assert!(said.contains(&format!("It couldn't change ~/.profile (it isn't writable): take out {lines} by hand.")), "{said}");
    assert!(!said.contains("take Kumi out of"), "{said}");
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap(), 1);
    let said = out.0.borrow().clone();
    assert!(
        said.contains(&format!("Couldn't take Kumi out of ~/.profile (Permission denied (os error 13)): take out {lines} by hand.")),
        "{said}"
    );
    assert_eq!(fs::read_to_string(&profile).unwrap(), rc);
    assert!(!home.join("app").exists());
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o644)).unwrap();
}
/// Kumi's block is found as install.sh finds it (CRLF endings, spaces after the marker), and one inside an if, for or
/// function body is left alone and named: taking it out could leave an empty body, which the shell refuses.
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_finds_kumis_block_as_install_sh_does_and_leaves_one_in_a_body() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let user = Path::new(&env["HOME"]);
    installed_kumi(home);
    let line = format!("export PATH=\"{}/bin:$PATH\"", home.display());
    put(user.join(".zshrc"), format!("alias ll='ls -l'\r\n\r\n{PATH_MARKER}\r\n{line}\r\n"));
    put(user.join(".zprofile"), format!("alias ll='ls -l'\n\n{PATH_MARKER}   \n{line}\n"));
    let in_if = format!("if [ -d /opt ]; then\n{PATH_MARKER}\n{line}\nfi\n");
    let in_function = format!("setup() {{\n  # Kumi\n  {PATH_MARKER}\n  {line}\n}}\n");
    put(user.join(".bashrc"), &in_if);
    put(user.join(".profile"), &in_function);
    let (dry, out) = io(&env);
    assert_eq!(uninstall_installed(dry, UninstallOptions { dry_run: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    for words in [
        "It would also take Kumi out of ~/.zshrc and ~/.zprofile.".to_string(),
        format!("It would leave Kumi's PATH line in ~/.bashrc alone (lines 2–3: {PATH_MARKER} / {line}): it sits inside an if, for or function there."),
        "It would leave Kumi's PATH line in ~/.profile alone (lines 3–4: ".into(),
    ] {
        assert!(said.contains(&words), "{words}: {said}");
    }
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap(), 0, "{}", out.0.borrow());
    assert_eq!(fs::read_to_string(user.join(".zshrc")).unwrap(), "alias ll='ls -l'\r\n");
    assert_eq!(fs::read_to_string(user.join(".zprofile")).unwrap(), "alias ll='ls -l'\n");
    assert_eq!(fs::read_to_string(user.join(".bashrc")).unwrap(), in_if);
    assert_eq!(fs::read_to_string(user.join(".profile")).unwrap(), in_function);
    let said = out.0.borrow().clone();
    assert!(
        said.contains(&format!("Left Kumi's PATH line in ~/.bashrc alone (lines 2–3: {PATH_MARKER} / {line}): it sits inside an if, for or function there; take it out by hand if you like.")),
        "{said}"
    );
}
/// sudo sets HOME to root's own while Kumi's launcher still finds Kumi's folder beside it: in the home folder of the
/// account that ran sudo, when an earlier installer put Kumi there. The account database's home folders count too,
/// and root leaves a folder that isn't root's alone (su sets no SUDO_USER).
#[tokio::test]
async fn uninstall_as_root_spares_the_home_folder_the_account_database_names() {
    let dir = tempfile::tempdir().unwrap();
    let theirs = dir.path().join("home-me");
    let roots = dir.path().join("root");
    installed_kumi(&theirs);
    put(theirs.join("bin/backup.sh"), "theirs");
    put(theirs.join("notes.txt"), "theirs");
    fs::create_dir_all(&roots).unwrap();
    let before = files_in(&theirs);
    let mut env = env(dir.path());
    env.insert("HOME".into(), roots.display().to_string());
    env.insert("KUMI_HOME".into(), theirs.display().to_string());
    // Under sudo: root, with root's home folder and that of the account that ran sudo.
    let sudo = Runner { root: true, homes: vec![roots.display().to_string(), theirs.display().to_string()] };
    assert_eq!(not_kumis_own_for(&env["KUMI_HOME"], &env, &sudo), Some(NotKumis::Home));
    for options in [UninstallOptions { yes: true, ..Default::default() }, UninstallOptions { all: true, yes: true, ..Default::default() }] {
        let (mut io, out) = io(&env);
        io.runner = Some(sudo.clone());
        assert_eq!(uninstall_installed(io, options).await.unwrap(), 1);
        assert!(out.0.borrow().contains(": it's your home folder, so the names Kumi uses there"), "{}", out.0.borrow());
        assert_eq!(files_in(&theirs), before);
    }
    // Root, on a folder that isn't root's, wherever it is.
    #[cfg(unix)]
    // SAFETY: geteuid has no preconditions. As root, the test's own folders are root's.
    if unsafe { libc::geteuid() } != 0 {
        let kumi = dir.path().join("elsewhere/.kumi");
        installed_kumi(&kumi);
        let before = files_in(&kumi);
        let mut there = env.clone();
        there.insert("KUMI_HOME".into(), kumi.display().to_string());
        let root = Runner { root: true, homes: vec![] };
        assert_eq!(not_kumis_own_for(&there["KUMI_HOME"], &there, &root), Some(NotKumis::NotRoots));
        assert_eq!(not_kumis_own_for(&there["KUMI_HOME"], &there, &Runner::default()), None);
        let (mut io, out) = io(&there);
        io.runner = Some(root);
        assert_eq!(uninstall_installed(io, UninstallOptions { yes: true, ..Default::default() }).await.unwrap(), 1);
        assert!(
            out.0.borrow().contains("as root (sudo): it belongs to another account. Run kumi uninstall without sudo. Nothing was removed."),
            "{}",
            out.0.borrow()
        );
        assert_eq!(files_in(&kumi), before);
    }
    // The account database answers for whoever runs this.
    let me = Runner::current(&Env::new());
    #[cfg(unix)]
    // SAFETY: geteuid has no preconditions.
    assert_eq!(me.root, unsafe { libc::geteuid() } == 0);
    assert!(me.homes.iter().all(|home| Path::new(home).is_absolute()), "{me:?}");
}
/// A Kumi folder the producer also uses (KUMI_HOME=~/.local, so the launcher is ~/.local/bin/kumi): with or without
/// --all, only Kumi's own entries go. Theirs in bin and downloads and beside them stay, Kumi's data stays (its names,
/// such as projects, could be theirs there), and so does the folder. In `bridges`, only the DAW folders Kumi's bridge
/// lifecycle wrote go: another app's, and a `reaper` folder without the lifecycle's files, stay with their `logs`.
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_from_a_folder_the_producer_also_uses_takes_only_kumis_own() {
    let theirs = [
        "bin/uv-tool",
        "downloads/mine.zip",
        "share/applications/x.desktop",
        "lib/python3/site.py",
        "state/history",
        "bridges/sync/logs/today.log",
        "bridges/reaper/discovery/peers.json",
    ];
    let data = ["auth.json", "kumi.db", "projects/0123456789abcdef0123456789abcdef/conversations/a.json"];
    for all in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut env = env(dir.path());
        let user = Path::new(&env["HOME"]).to_path_buf();
        let local = user.join(".local");
        env.insert("KUMI_HOME".into(), local.display().to_string());
        installed_kumi(&local);
        put(local.join("app.previous/kumi"), "Kumi's");
        put(local.join("downloads/kumi-aarch64-apple-darwin.tar.gz"), "Kumi's");
        put(local.join("tools/ffmpeg/ffmpeg"), "Kumi's");
        put(local.join("bridges/renoise/bridge.secret"), "Kumi's");
        put(local.join("bridges/renoise/logs/bridge.log"), "Kumi's");
        for file in theirs {
            put(local.join(file), "theirs");
        }
        for file in data {
            put(local.join(file), "Kumi's data");
        }
        put(user.join(".profile"), format!("alias ll='ls -l'\n\n{PATH_MARKER}\nexport PATH=\"{}/bin:$PATH\"\n", local.display()));
        let kept = "conversations, notes, recipes, sign-ins, library and references";
        let (dry, out) = io(&env);
        assert_eq!(uninstall_installed(dry, UninstallOptions { all, dry_run: true, ..Default::default() }).await.unwrap(), 0);
        let said = out.0.borrow().clone();
        for words in [
            "): app, app.previous, bin/kumi, downloads/kumi-aarch64-apple-darwin.tar.gz, tools, bridges/renoise.".to_string(),
            "~/.local/bin and ~/.local/downloads stay: they hold more than Kumi put there.".into(),
            if all {
                format!("~/.local also holds what isn't Kumi's, so --all would leave your {kept} there too (auth.json, kumi.db and projects): delete them by hand.")
            } else {
                format!("Your {kept} would stay in ~/.local, beside what isn't Kumi's (auth.json, kumi.db and projects).")
            },
        ] {
            assert!(said.contains(&words), "{words}: {said}");
        }
        let (removing, out) = io(&env);
        assert_eq!(
            uninstall_installed(removing, UninstallOptions { all, yes: true, ..Default::default() }).await.unwrap(),
            0,
            "{}",
            out.0.borrow()
        );
        for file in theirs {
            assert_eq!(fs::read_to_string(local.join(file)).unwrap(), "theirs", "{file}");
        }
        for file in data {
            assert_eq!(fs::read_to_string(local.join(file)).unwrap(), "Kumi's data", "{file}");
        }
        for gone in ["app", "app.previous", "bin/kumi", "downloads/kumi-aarch64-apple-darwin.tar.gz", "tools", "bridges/renoise"] {
            assert!(!local.join(gone).exists(), "{gone}");
        }
        assert_eq!(fs::read_to_string(user.join(".profile")).unwrap(), "alias ll='ls -l'\n");
        let said = out.0.borrow().clone();
        assert!(
            said.contains(&format!(
                "Your {kept} are still in ~/.local, beside what isn't Kumi's (auth.json, kumi.db and projects): delete them by hand to remove them too."
            )) && !said.contains("delete that folder"),
            "{said}"
        );
    }
}
/// When Kumi's app itself can't go, its launcher and PATH lines stay too: a second run then works, and what it says
/// is still there is.
#[cfg(unix)]
#[tokio::test]
async fn uninstall_keeps_the_launcher_and_path_lines_while_the_app_stays() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions. Root removes anything.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let user = Path::new(&env["HOME"]);
    installed_kumi(home);
    put(home.join("app.previous/kumi"), "Kumi's");
    put(home.join("downloads/kumi-aarch64-apple-darwin.tar.gz"), "Kumi's");
    let rc = format!("{PATH_MARKER}\nexport PATH=\"{}/bin:$PATH\"\n", home.display());
    put(user.join(".profile"), &rc);
    // An app folder this account can't empty.
    fs::set_permissions(home.join("app"), fs::Permissions::from_mode(0o555)).unwrap();
    let (removing, out) = io(&env);
    let code = uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap();
    fs::set_permissions(home.join("app"), fs::Permissions::from_mode(0o755)).unwrap();
    let said = out.0.borrow().clone();
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("/app couldn't go (Permission denied (os error 13)). Kumi's launcher (")
            && said.contains(
                "/bin/kumi) and PATH lines stay until then: once that's sorted (if another account owns it: sudo chown -R $USER "
            )
            && said.contains("run kumi uninstall again, or take out "),
        "{said}"
    );
    assert!(home.join("bin/kumi").exists() && home.join("app/package.json").exists());
    assert!(!home.join("app.previous").exists() && !home.join("downloads").exists());
    assert_eq!(fs::read_to_string(user.join(".profile")).unwrap(), rc);
    // Sorted, the second run finishes.
    let (again, out) = io(&env);
    assert_eq!(uninstall_installed(again, UninstallOptions { yes: true, ..Default::default() }).await.unwrap(), 0, "{}", out.0.borrow());
    assert!(!home.join("app").exists() && !home.join("bin").exists());
    assert_eq!(fs::read_to_string(user.join(".profile")).unwrap(), "");
}
/// An installer given a HOME ending in / wrote the PATH line with …//.kumi/bin: it's Kumi's all the same.
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_knows_a_path_line_written_from_a_home_ending_in_a_slash() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(dir.path());
    let user = dir.path().join("user");
    let home = user.join(".kumi");
    env.insert("HOME".into(), format!("{}/", user.display()));
    env.insert("KUMI_HOME".into(), home.display().to_string());
    installed_kumi(&home);
    put(user.join(".zshrc"), format!("alias ll='ls -l'\n\n{PATH_MARKER}\nexport PATH=\"{}//.kumi/bin:$PATH\"\n", user.display()));
    put(user.join(".profile"), format!("{PATH_MARKER}\nexport PATH=\"$HOME/.kumi//bin:$PATH\"\n"));
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap(), 0, "{}", out.0.borrow());
    assert_eq!(fs::read_to_string(user.join(".zshrc")).unwrap(), "alias ll='ls -l'\n");
    assert_eq!(fs::read_to_string(user.join(".profile")).unwrap(), "");
}
/// A folder counts as Kumi's alone only by its own name, .kumi, once links are followed; what's in it can't tell, as
/// Kumi's names (bin, projects…) are everyone's. And even there, only Kumi's entries go from bin, downloads and the
/// rest, and the folder only once it's empty.
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_all_takes_a_folder_whole_only_when_it_is_kumis_alone() {
    let kept = "conversations, notes, recipes, sign-ins, library and references";
    // A ~/.local that held only the producer's own scripts in bin before Kumi, and a ~/.kumi that is a link to it.
    for linked in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut env = env(dir.path());
        let user = Path::new(&env["HOME"]).to_path_buf();
        let local = user.join(".local");
        installed_kumi(&local);
        put(local.join("bin/uv-tool"), "theirs");
        put(local.join("bin/my-script"), "theirs");
        put(local.join("auth.json"), "Kumi's data");
        let home = if linked {
            std::os::unix::fs::symlink(&local, user.join(".kumi")).unwrap();
            user.join(".kumi")
        } else {
            local.clone()
        };
        env.insert("KUMI_HOME".into(), home.display().to_string());
        let (dry, out) = io(&env);
        assert_eq!(uninstall_installed(dry, UninstallOptions { all: true, dry_run: true, ..Default::default() }).await.unwrap(), 0);
        let said = out.0.borrow().clone();
        assert!(said.contains("): app, bin/kumi.") && said.contains("/bin stays: it holds more than Kumi put there."), "{said}");
        assert!(said.contains(&format!("--all would leave your {kept} there too (auth.json)")), "{said}");
        let (removing, out) = io(&env);
        assert_eq!(uninstall_installed(removing, UninstallOptions { all: true, yes: true, ..Default::default() }).await.unwrap(), 0);
        for file in ["bin/uv-tool", "bin/my-script"] {
            assert_eq!(fs::read_to_string(local.join(file)).unwrap(), "theirs", "{file} ({linked})");
        }
        assert!(local.join("auth.json").exists() && !local.join("app").exists() && !local.join("bin/kumi").exists());
        assert!(!out.0.borrow().contains("delete that folder"), "{}", out.0.borrow());
    }
    // ~/.kumi itself: --all takes Kumi's data too, but a file of the producer's in its bin keeps that, and the folder.
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    installed_kumi(home);
    put(home.join("bin/mine.sh"), "theirs");
    put(home.join("downloads/kumiko.zip"), "theirs");
    put(home.join("downloads/kumi-photos.zip"), "theirs");
    put(home.join("downloads/kumi-aarch64-apple-darwin.tar.gz"), "Kumi's");
    put(home.join("auth.json"), "Kumi's data");
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { all: true, yes: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    assert_eq!(
        files_in(home).into_iter().map(|(file, _)| file).collect::<Vec<_>>(),
        [
            home.join("bin/mine.sh").display().to_string(),
            home.join("downloads/kumi-photos.zip").display().to_string(),
            home.join("downloads/kumiko.zip").display().to_string()
        ]
    );
    assert!(
        said.contains("Kumi is removed, with everything it kept, freeing ") && said.contains(" stays, with what isn't Kumi's in it."),
        "{said}"
    );
    // With nothing else in it, it goes whole.
    fs::remove_dir_all(home).unwrap();
    installed_kumi(home);
    put(home.join("auth.json"), "Kumi's data");
    let (removing, _) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { all: true, yes: true, ..Default::default() }).await.unwrap(), 0);
    assert!(!home.exists());
}
/// On Windows, PATH names Kumi's bin folder itself: while that stays, holding the producer's own (uv's .local\bin,
/// say), so does its PATH entry. Elsewhere Kumi's own lines in the startup files go, as they're only Kumi's.
#[tokio::test]
async fn a_bin_folder_that_stays_keeps_its_windows_path_entry() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(dir.path());
    let local = Path::new(&env["HOME"]).join(".local");
    env.insert("KUMI_HOME".into(), local.display().to_string());
    installed_kumi(&local);
    put(local.join("bin").join(executable_name("uv")), "theirs");
    let (dry, out) = io(&env);
    assert_eq!(uninstall_installed(dry, UninstallOptions { dry_run: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    assert!(said.contains("bin stays: it holds more than Kumi put there."), "{said}");
    if cfg!(windows) {
        assert!(
            said.contains("would stay in your PATH, as it holds more than Kumi put there.") && !said.contains("take Kumi out of your PATH"),
            "{said}"
        );
    } else {
        assert!(!said.contains("your PATH"), "{said}");
    }
}
/// What's left of an app that lost its program doesn't run, so neither would kumi uninstall: only the way by hand is
/// offered then.
#[cfg(unix)]
#[tokio::test]
async fn uninstall_offers_a_second_run_only_while_the_app_still_runs() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions. Root removes anything.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    installed_kumi(home);
    fs::remove_file(home.join("app/kumi")).unwrap();
    fs::set_permissions(home.join("app"), fs::Permissions::from_mode(0o555)).unwrap();
    let (removing, out) = io(&env);
    let code = uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap();
    fs::set_permissions(home.join("app"), fs::Permissions::from_mode(0o755)).unwrap();
    let said = out.0.borrow().clone();
    assert_eq!(code, 1, "{said}");
    assert!(
        said.contains("and PATH lines stay, but what's left can't uninstall itself: take out ")
            && said.contains(", the launcher and the two lines under ")
            && !said.contains("run kumi uninstall again"),
        "{said}"
    );
}
/// Even in a folder named .kumi, --all takes only Kumi's names: what the producer put there themselves stays, named,
/// and keeps the folder.
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_all_keeps_what_the_producer_put_in_kumis_folder() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(dir.path());
    let home = Path::new(&env["HOME"]).join("Studio/.kumi");
    env.insert("KUMI_HOME".into(), home.display().to_string());
    installed_kumi(&home);
    for (file, text) in
        [("my-notes.txt", "theirs"), ("my-stems/kick.wav", "theirs"), ("auth.json", "Kumi's data"), ("projects/x.json", "Kumi's data")]
    {
        put(home.join(file), text);
    }
    let (dry, out) = io(&env);
    assert_eq!(uninstall_installed(dry, UninstallOptions { all: true, dry_run: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    assert!(said.contains("kumi uninstall --all would remove everything of Kumi's from ~/Studio/.kumi ("), "{said}");
    assert!(said.contains("my-notes.txt and my-stems stay in ~/Studio/.kumi: they aren't Kumi's."), "{said}");
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { all: true, yes: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    assert_eq!(
        files_in(&home).into_iter().map(|(file, _)| file).collect::<Vec<_>>(),
        [home.join("my-notes.txt").display().to_string(), home.join("my-stems/kick.wav").display().to_string()]
    );
    assert!(said.contains("my-notes.txt and my-stems stay in ~/Studio/.kumi: they aren't Kumi's."), "{said}");
    assert!(said.contains("~/Studio/.kumi stays, with what isn't Kumi's in it."), "{said}");
}
/// A second run is offered only while it would run: Kumi's program still in app, in a folder that still passes as
/// Kumi's. Here the app keeps its program but the folder has lost what marks it as Kumi's (its package.json went
/// with app.previous), so only the way by hand is offered.
#[cfg(unix)]
#[tokio::test]
async fn uninstall_offers_a_second_run_only_while_kumi_would_still_take_it() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions. Root removes anything.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    installed_kumi(home);
    fs::rename(home.join("app/package.json"), home.join("app.previous.json")).unwrap();
    put(home.join("app.previous/package.json"), fs::read(home.join("app.previous.json")).unwrap());
    fs::remove_file(home.join("app.previous.json")).unwrap();
    put(home.join("app.previous/kumi"), "Kumi's");
    fs::set_permissions(home.join("app"), fs::Permissions::from_mode(0o555)).unwrap();
    let (removing, out) = io(&env);
    let code = uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap();
    fs::set_permissions(home.join("app"), fs::Permissions::from_mode(0o755)).unwrap();
    let said = out.0.borrow().clone();
    assert_eq!(code, 1, "{said}");
    assert!(home.join("app/kumi").is_file() && !home.join("app.previous").exists(), "{said}");
    assert!(
        said.contains("and PATH lines stay, but what's left can't uninstall itself: take out ")
            && !said.contains("run kumi uninstall again"),
        "{said}"
    );
}
/// A folder holding only Kumi's, but not named .kumi (KUMI_HOME=~/Music/kumi): --all leaves Kumi's data, and says why
/// as it is, not that the folder holds what isn't Kumi's.
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_all_says_a_folder_of_only_kumis_isnt_named_kumi() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(dir.path());
    let home = Path::new(&env["HOME"]).join("Music/kumi");
    env.insert("KUMI_HOME".into(), home.display().to_string());
    installed_kumi(&home);
    put(home.join("auth.json"), "Kumi's data");
    let kept = "conversations, notes, recipes, sign-ins, library and references";
    let (dry, out) = io(&env);
    assert_eq!(uninstall_installed(dry, UninstallOptions { all: true, dry_run: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    assert!(
        said.contains(&format!(
            "~/Music/kumi isn't named .kumi, so --all would leave your {kept} there too (auth.json): delete them by hand."
        )),
        "{said}"
    );
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { all: true, yes: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    assert!(
        said.contains(&format!("Your {kept} are still in ~/Music/kumi (auth.json): delete them by hand to remove them too.")),
        "{said}"
    );
    assert!(!said.contains("what isn't Kumi's") && home.join("auth.json").exists() && !home.join("app").exists(), "{said}");
}

/// The DAW used last (`last-daw`) and REAPER's and Renoise's bridges' files (`bridges/`) are Kumi's: `--all` takes a
/// folder holding them whole, as it always took a Live-only producer's. (On Windows a detached cmd removes them once
/// Kumi has exited; the dry run below says the same there.)
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_all_takes_the_daw_used_last_and_the_other_daws_bridge_files_too() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    for part in ["app", "bin"] {
        fs::create_dir_all(home.join(part)).unwrap();
    }
    installed_kumi(home);
    put(home.join("last-daw"), "reaper");
    put(home.join("bridges/reaper/bridge.secret"), "secret");
    put(home.join("bridges/reaper/logs/reaper-1.log"), "log");
    let (io, out) = io(&env);
    assert_eq!(uninstall_installed(io, UninstallOptions { all: true, yes: true, ..Default::default() }).await.unwrap(), 0);
    assert!(!home.exists(), "{}", out.0.borrow());
    assert!(!out.0.borrow().contains("isn't Kumi's"), "{}", out.0.borrow());
}

/// A DAW whose folder is gone (Renoise's preferences deleted) doesn't stop the uninstall halfway: Kumi's tool comes out
/// as its receipt says, and Kumi goes; nobody is asked to open Renoise to uninstall Kumi.
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_carries_on_when_a_daws_folder_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]).to_path_buf();
    for part in ["app", "bin"] {
        fs::create_dir_all(home.join(part)).unwrap();
    }
    installed_kumi(&home);
    // Kumi's tool was in Renoise, and Renoise's preferences folder has since been deleted.
    let tools = dir.path().join("Renoise/V3.5.4/Scripts/Tools");
    env.insert("KUMI_RENOISE_PREFS_DIR".into(), dir.path().join("Renoise").display().to_string());
    let paths = daw_bridge::paths::BridgePaths::new(&home, daw_bridge::daw::Daw::Renoise);
    fs::create_dir_all(&paths.root).unwrap();
    let receipt = daw_bridge::lifecycle::Receipt {
        schema: daw_bridge::lifecycle::RECEIPT_SCHEMA.into(),
        daw: "renoise".into(),
        status: "installed".into(),
        target: tools.join("com.kumi.Bridge.xrnx").display().to_string(),
        reference: tools.join("com.kumi.Bridge.xrnx/bridge-reference.json").display().to_string(),
        generation: Some("g1".into()),
        version: Some("1.0.0".into()),
        sha256: None,
        previous: vec![],
        copies: vec![],
        updated_ms: 0,
        history: vec![],
    };
    put(paths.receipt(), serde_json::to_string(&receipt).unwrap());
    let (io, out) = io(&env);
    assert_eq!(
        uninstall_installed(io, UninstallOptions { all: false, yes: true, ..Default::default() }).await.unwrap(),
        0,
        "{}",
        out.0.borrow()
    );
    for part in ["app", "bin"] {
        assert!(!home.join(part).exists(), "{part}: {}", out.0.borrow());
    }
    assert!(!out.0.borrow().contains("open Renoise"), "{}", out.0.borrow());
}

/// On every system, `last-daw` and `bridges/` count as Kumi's: `--all` would take the whole folder (said by the dry run,
/// which removes nothing, so it runs on Windows too).
#[tokio::test]
async fn the_daw_used_last_and_the_other_daws_bridge_files_count_as_kumis() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    for part in ["app", "bin"] {
        fs::create_dir_all(home.join(part)).unwrap();
    }
    installed_kumi(home);
    put(home.join("last-daw"), "reaper");
    put(home.join("bridges/renoise/bridge.secret"), "secret");
    let (io, out) = io(&env);
    assert_eq!(uninstall_installed(io, UninstallOptions { all: true, dry_run: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    assert!(said.contains("kumi uninstall --all would remove ") && !said.contains("isn't Kumi's"), "{said}");
    assert!(home.join("last-daw").exists() && home.join("bridges").exists(), "a dry run removes nothing");
}

/// Uninstalling takes out what's Kumi's in REAPER's and Renoise's bridge folders and keeps what the lifecycle promised:
/// the copies of Kumi's bridge the producer had changed (`changed`) always, and catch-up's baselines (`snapshots`) until
/// --all. A DAW's folder with nothing else in it goes whole.
#[cfg(not(windows))]
#[tokio::test]
async fn uninstall_keeps_the_changed_copies_and_until_all_the_baselines_in_the_daws_bridge_folders() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]);
    let reaper = home.join("bridges/reaper");
    let setup = || {
        installed_kumi(home);
        for part in ["generations/0.1.0-1/reaper_kumi.dylib", "discovery/a.json", "logs/bridge.log", "listening/j-s.wav"] {
            put(reaper.join(part), "kumi's");
        }
        for file in ["bridge.secret", "lifecycle.lock", ".install-receipt.json.tmp-0123456789ab"] {
            put(reaper.join(file), "kumi's");
        }
        put(reaper.join("changed/reaper_kumi.dylib-1700000000000"), "the producer's own build");
        put(reaper.join("snapshots/reaper-0123/last-seen.json"), "{}");
        put(home.join("bridges/renoise/bridge.secret"), "kumi's");
        put(home.join("bridges/renoise/logs/bridge.log"), "kumi's");
    };
    setup();
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { yes: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    for part in
        ["generations", "discovery", "logs", "listening", "bridge.secret", "lifecycle.lock", ".install-receipt.json.tmp-0123456789ab"]
    {
        assert!(!reaper.join(part).exists(), "{part}: {said}");
    }
    assert!(!home.join("bridges/renoise").exists(), "nothing else in it: it goes whole. {said}");
    assert_eq!(fs::read_to_string(reaper.join("changed/reaper_kumi.dylib-1700000000000")).unwrap(), "the producer's own build");
    assert!(reaper.join("snapshots/reaper-0123/last-seen.json").exists(), "the baselines stay without --all: {said}");
    assert!(said.contains("The copies of Kumi's bridges you had changed stay in ") && said.contains("changed"), "{said}");
    // --all takes the baselines too, never the producer's copies.
    setup();
    let (removing, out) = io(&env);
    assert_eq!(uninstall_installed(removing, UninstallOptions { all: true, yes: true, ..Default::default() }).await.unwrap(), 0);
    let said = out.0.borrow().clone();
    assert!(!reaper.join("snapshots").exists(), "{said}");
    assert!(reaper.join("changed/reaper_kumi.dylib-1700000000000").exists(), "{said}");
    assert!(home.exists(), "the producer's copies keep Kumi's folder: {said}");
}

/// `kumi update` brings Renoise's tool from the new Kumi's own folder, not from the Kumi that runs the update; going back
/// to a Kumi that doesn't carry it says the tool stays, where, and how to take it out.
#[tokio::test]
async fn an_update_brings_renoises_tool_from_the_new_kumi_and_going_back_says_it_stays() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(dir.path());
    let home = Path::new(&env["KUMI_HOME"]).to_path_buf();
    put(home.join("app/package.json"), json!({"version":KUMI_VERSION}).to_string());
    put(home.join("app").join(executable_name("kumi")), "earlier");
    // One component at a time, as Kumi builds the paths it shows: Windows shows `\` throughout.
    let prefs = dir.path().join("Renoise").join("V3.5.4");
    let tools = prefs.join("Scripts").join("Tools");
    fs::create_dir_all(&tools).unwrap();
    env.insert("KUMI_RENOISE_PREFS_DIR".into(), prefs.display().to_string());
    let tool = |version: &str| format!("return \"{version}\"\n");
    let folder = daw_bridge::install::RENOISE_TOOL_FOLDER;
    // The tool the Kumi before the update put in Renoise.
    let before = dir.path().join("before").join(folder);
    put(before.join("payload/version.lua"), tool("1.0.0"));
    let target = daw_bridge::lifecycle::Target {
        daw: daw_bridge::daw::Daw::Renoise,
        dir: tools.clone(),
        name: folder.into(),
        folder: true,
        reference: tools.join(folder).join(daw_bridge::install::RENOISE_REFERENCE),
    };
    daw_bridge::lifecycle::Lifecycle::new(&home, target).install(&before, "1.0.0").unwrap();
    let version_lua = format!("daws/renoise/{folder}/payload/version.lua");
    let bytes = fake_release_carrying(dir.path(), "99.0.0", None, &[(&version_lua, &tool("2.0.0"))]);
    let mut good = manifest("99.0.0");
    good["sha256"] = json!(hex::encode(Sha256::digest(&bytes)));
    let (mut updated, out) = io(&env);
    updated.fetcher = Some(Rc::new(Serve { manifest: good, bytes, status: 200, offline: false, calls: RefCell::new(vec![]) }));
    updated.others = Some(Rc::new(|| async { 0 }.boxed_local()));
    updated.run = Some(Rc::new(|command, args, cwd| async move { run_program(&command, &args, cwd.as_deref()).await }.boxed_local()));
    assert_eq!(update_installed(updated).await.unwrap(), 0, "{}", out.0.borrow());
    assert_eq!(fs::read_to_string(tools.join(folder).join("payload/version.lua")).unwrap(), tool("2.0.0"), "{}", out.0.borrow());
    let (rollback, out) = io(&env);
    assert_eq!(rollback_installed(rollback).await.unwrap(), 0, "{}", out.0.borrow());
    let said = out.0.borrow().clone();
    assert!(
        said.contains(&format!(
            "Kumi's tool stays in Renoise ({}): Kumi {KUMI_VERSION} doesn't know Renoise",
            tools.join(folder).display()
        )),
        "{said}"
    );
    assert!(said.contains("Delete that folder to take it out."), "{said}");
    assert!(tools.join(folder).exists(), "going back leaves the tool where it is");
}
