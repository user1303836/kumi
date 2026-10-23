use ableton_mcp_server::{
    bridge::{extension_channel::read_extension_endpoint, extension_launcher::*, extension_setup::*},
    live::*,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};
use tokio::task::LocalSet;
#[test]
fn process_scan_identifies_kumi_shared_hosts_and_lives_own() {
    let folder = "/Users/p/Library/Application Support/kumi/live-extension";
    let windows = "C:\\Users\\p\\AppData\\Local\\kumi\\live-extension";
    let kumi = format!(
        "/Applications/Ableton Live 12.app/Contents/Helpers/ExtensionHost/node -e globalThis.__kumiLaunchedHost kumi-storage:{}",
        URL_SAFE_NO_PAD.encode(folder)
    );
    let own = "/Applications/Ableton Live 12.app/Contents/Helpers/ExtensionHost/node --installed";
    let win = format!(
        r"C:\ProgramData\Ableton\Live 12\Program\ExtensionHost\node.exe -e __kumiLaunchedHost kumi-storage:{}",
        URL_SAFE_NO_PAD.encode(windows)
    );
    assert_eq!(parse_extension_hosts(&format!("{kumi}\n/usr/bin/other\n")), ExtensionHosts { kumi: vec![folder.into()], live: false });
    assert_eq!(parse_extension_hosts(&win), ExtensionHosts { kumi: vec![windows.into()], live: false });
    assert_eq!(parse_extension_hosts(own), ExtensionHosts { kumi: vec![], live: true });
    assert_eq!(parse_extension_hosts(&format!("{kumi}\r\n{own}")), ExtensionHosts { kumi: vec![folder.into()], live: true });
    assert_eq!(
        parse_extension_hosts("ExtensionHost/node -e __kumiLaunchedHost kumi-storage:Z"),
        ExtensionHosts { kumi: vec![PathBuf::new()], live: false }
    );
    assert!(find_extension_bundle().is_some());
}
fn endpoint(directory: &Path, pid: u32) {
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(
        directory.join("endpoint.json"),
        serde_json::to_vec(
            &json!({"host":"127.0.0.1","port":1,"pid":pid,"extensionVersion":"test","registryHash":"x","apiVersion":"1.0.0","startedAt":0}),
        )
        .unwrap(),
    )
    .unwrap();
}
#[tokio::test]
async fn answering_shared_live_host_and_missing_bundle_outcomes() {
    let root = tempfile::tempdir().unwrap();
    let answering = root.path().join("answering");
    endpoint(&answering, std::process::id());
    let mut options = LaunchOptions::new(&answering);
    options.scan = ExtensionScan::Disabled;
    assert_eq!(launch_extension(options).await.unwrap(), LaunchOutcome::Answering);
    let storage = root.path().join("other");
    let observed = Rc::new(RefCell::new(None));
    let shared = observed.clone();
    let folder = answering.clone();
    let mut options = LaunchOptions::new(&storage);
    options.scan = ExtensionScan::Function(Rc::new(move || ExtensionHosts { kumi: vec![folder.clone()], live: false }));
    options.on_shared = Some(Rc::new(move |path| *shared.borrow_mut() = Some(path.to_path_buf())));
    assert_eq!(launch_extension(options).await.unwrap(), LaunchOutcome::Shared);
    assert_eq!(observed.borrow().as_ref(), Some(&answering));
    let logs = Rc::new(RefCell::new(vec![]));
    let captured = logs.clone();
    let mut options = LaunchOptions::new(&storage);
    options.scan = ExtensionScan::Function(Rc::new(|| ExtensionHosts { kumi: vec![], live: true }));
    options.log = Some(Rc::new(move |line| captured.borrow_mut().push(line.to_string())));
    assert_eq!(launch_extension(options).await.unwrap(), LaunchOutcome::LiveHost);
    assert!(logs.borrow()[0].contains("Live runs its own Extension Host"));
    let mut options = LaunchOptions::new(&storage);
    options.scan = ExtensionScan::Disabled;
    options.extension = Some(root.path().join("no-extension"));
    assert_eq!(launch_extension(options).await.unwrap(), LaunchOutcome::Unavailable);
}
#[cfg(unix)]
fn fake_host(directory: &Path, answer: bool) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(directory.join("ExtensionHostNodeModule.node"), "").unwrap();
    let node = std::process::Command::new("node").args(["-p", "process.execPath"]).output().unwrap();
    assert!(node.status.success());
    let node = String::from_utf8(node.stdout).unwrap();
    let script = directory.join("fake-host.cjs");
    let answer = if answer {
        "require('fs').writeFileSync(require('path').join(storage, 'endpoint.json'), JSON.stringify({host:'127.0.0.1',port:1,pid:process.pid,extensionVersion:'test',registryHash:'x',apiVersion:'1.0.0',startedAt:Date.now(),launched:process.env.KUMI_LAUNCHED_HOST,config}));"
    } else {
        ""
    };
    std::fs::write(&script,format!("const config=JSON.parse(process.argv[process.argv.indexOf('-e')+2]);const storage=config.extensions[0].storageDirectory;{answer}setTimeout(()=>{{}},60000);")).unwrap();
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\"'\"'"));
    let path = directory.join("node");
    std::fs::write(&path, format!("#!/bin/sh\nexec {} {} \"$@\"\n", quote(node.trim()), quote(&script.to_string_lossy()))).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
#[cfg(unix)]
struct KillPid(Option<u32>);
#[cfg(unix)]
impl Drop for KillPid {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
    }
}
#[cfg(unix)]
#[tokio::test]
async fn detached_launch_creates_secret_configuration_and_releases_lock() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let host = root.path().join("ExtensionHost");
    fake_host(&host, true);
    let storage = root.path().join("storage");
    let lock = root.path().join("launch.lock");
    let mut options = LaunchOptions::new(&storage);
    options.live_app = Some(host);
    options.extension = find_extension_bundle();
    options.scan = ExtensionScan::Disabled;
    options.lock_path = Some(lock.clone());
    options.wait_ms = Some(3000.0);
    assert_eq!(launch_extension(options).await.unwrap(), LaunchOutcome::Started);
    let endpoint = read_extension_endpoint(&storage).unwrap();
    let _kill = KillPid(endpoint["pid"].as_u64().map(|pid| pid as u32));
    assert_eq!(endpoint["launched"], "1");
    assert_eq!(endpoint["config"]["extensions"][0]["storageDirectory"], storage.to_string_lossy().as_ref());
    assert!(storage.join("tmp").is_dir());
    assert!(storage.join("extension-host.log").is_file());
    let secret = std::fs::read_to_string(storage.join("secret")).unwrap();
    assert!(secret.trim().len() >= 32);
    assert_eq!(std::fs::metadata(storage.join("secret")).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(!lock.exists());
}
#[cfg(unix)]
#[tokio::test]
async fn a_host_stopped_by_a_signal_is_seen_at_once() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let host = root.path().join("ExtensionHost");
    std::fs::create_dir_all(&host).unwrap();
    std::fs::write(host.join("ExtensionHostNodeModule.node"), "").unwrap();
    // Killed, so it has no exit code.
    std::fs::write(host.join("node"), "#!/bin/sh\nkill -KILL $$\n").unwrap();
    std::fs::set_permissions(host.join("node"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let logs = Rc::new(RefCell::new(vec![]));
    let observed = logs.clone();
    let mut options = LaunchOptions::new(root.path().join("storage"));
    options.live_app = Some(host);
    options.extension = find_extension_bundle();
    options.lock_path = Some(root.path().join("launch.lock"));
    options.scan = ExtensionScan::Disabled;
    options.wait_ms = Some(5000.0);
    options.log = Some(Rc::new(move |line| observed.borrow_mut().push(line.to_string())));
    let started = std::time::Instant::now();
    assert_eq!(launch_extension(options).await.unwrap(), LaunchOutcome::Failed);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert!(logs.borrow().iter().any(|line| line.contains("the Extension Host stopped")), "{:?}", logs.borrow());
}
#[cfg(unix)]
#[tokio::test]
async fn another_launch_lock_is_waited_for_and_unreachable_host_is_stopped() {
    let root = tempfile::tempdir().unwrap();
    let host = root.path().join("ExtensionHost");
    fake_host(&host, false);
    let storage = root.path().join("storage");
    let shared = root.path().join("shared");
    let lock = root.path().join("launch.lock");
    std::fs::write(&lock, "").unwrap();
    let other = shared.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        endpoint(&other, std::process::id());
    });
    let returned = Rc::new(RefCell::new(None));
    let captured = returned.clone();
    let scanned = shared.clone();
    let mut options = LaunchOptions::new(&storage);
    options.live_app = Some(host.clone());
    options.extension = find_extension_bundle();
    options.lock_path = Some(lock.clone());
    options.wait_ms = Some(1500.0);
    options.scan = ExtensionScan::Function(Rc::new(move || ExtensionHosts { kumi: vec![scanned.clone()], live: false }));
    options.on_shared = Some(Rc::new(move |path| *captured.borrow_mut() = Some(path.to_path_buf())));
    assert_eq!(launch_extension(options).await.unwrap(), LaunchOutcome::Shared);
    assert_eq!(returned.borrow().as_ref(), Some(&shared));
    assert!(lock.exists());
    std::fs::remove_file(&lock).unwrap();
    let logs = Rc::new(RefCell::new(vec![]));
    let observed = logs.clone();
    let mut options = LaunchOptions::new(&storage);
    options.live_app = Some(host);
    options.extension = find_extension_bundle();
    options.lock_path = Some(lock.clone());
    options.scan = ExtensionScan::Disabled;
    options.wait_ms = Some(150.0);
    options.log = Some(Rc::new(move |line| observed.borrow_mut().push(line.to_string())));
    assert_eq!(launch_extension(options).await.unwrap(), LaunchOutcome::Failed);
    assert!(read_extension_endpoint(&storage).is_none());
    assert!(logs.borrow().iter().any(|line| line.contains("didn't reach Live in time; stopping it")));
    assert!(!lock.exists());
}
#[derive(Clone)]
struct StatusAdapter(Rc<RefCell<LiveStatus>>);
impl LiveAdapter for StatusAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(self.0.borrow().clone())
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        Err(LiveError::error("unused"))
    }
    fn get(&self, _: &LiveRef) -> Result<Option<Value>, LiveError> {
        Err(LiveError::error("unused"))
    }
    fn invoke(&self, _: &LiveInvocation) -> Result<Value, LiveError> {
        Err(LiveError::error("unused"))
    }
    fn subscribe(&self, _: LiveListener) -> Result<Unsubscribe, LiveError> {
        Ok(Box::new(|| {}))
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.status()
    }
}
#[async_trait(?Send)]
impl AsyncLiveAdapter for StatusAdapter {
    async fn snapshot_async(&self, _: Option<&LiveOperationContext>, _: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.snapshot()
    }
    async fn discover_async(&self, _: &LiveDiscoveryRequest, _: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        Err(LiveError::error("unused"))
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.invoke(i)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.status()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}
#[tokio::test]
async fn retry_launch_requires_real_live_and_failed_host_waits_for_new_epoch() {
    LocalSet::new().run_until(async{
 let root=tempfile::tempdir().unwrap();let status:LiveStatus=serde_json::from_value(json!({"connected":true,"adapter":"remote-script","epoch":1,"protocol":"ableton-live/v1","capabilities":[],"provenance":"fake-live"})).unwrap();let state=Rc::new(RefCell::new(status));let remote=Rc::new(StatusAdapter(state.clone()));let launches=Rc::new(Cell::new(0));let calls=launches.clone();let mut setup=ExtensionSetup::new(root.path());setup.retry_ms=Some(5.0);setup.launcher=Some(Rc::new(move |_|{calls.set(calls.get()+1);Box::pin(async{Ok(LaunchOutcome::Failed)})}));let adapter=with_extension(remote,setup);tokio::time::sleep(Duration::from_millis(30)).await;assert_eq!(launches.get(),0);state.borrow_mut().provenance=Some(LiveProvenance::RealLive);tokio::time::sleep(Duration::from_millis(40)).await;assert_eq!(launches.get(),1);state.borrow_mut().epoch=Some(2);tokio::time::sleep(Duration::from_millis(40)).await;assert_eq!(launches.get(),2);adapter.close().await.unwrap();state.borrow_mut().epoch=Some(3);tokio::time::sleep(Duration::from_millis(30)).await;assert_eq!(launches.get(),2);
}).await;
}
