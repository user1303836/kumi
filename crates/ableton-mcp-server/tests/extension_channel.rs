//! Interoperability with the Live extension.
use ableton_mcp_server::{
    bridge::{extension_channel::*, live_extension_folders::*, router::*},
    live::*,
};
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::LocalSet,
};
struct Peer {
    _child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    folder: tempfile::TempDir,
}
impl Peer {
    async fn start() -> Self {
        let folder = tempfile::tempdir().unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut child = Command::new("node")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/extension-peer.mjs"))
            .arg(folder.path())
            .arg(repo)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("extension interoperability tests require Node, as Live's Extension Host does");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        // Up to 30 s: on a busy Windows runner, Node starting beside six others can take more than 5.
        for _ in 0..3000 {
            if read_extension_endpoint(folder.path()).is_some() {
                return Self { _child: child, stdin, stdout, folder };
            }
            if let Some(code) = child.try_wait().unwrap() {
                use tokio::io::AsyncReadExt;
                let mut stderr = String::new();
                child.stderr.take().unwrap().read_to_string(&mut stderr).await.unwrap();
                panic!("extension exited {code}: {stderr}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("extension did not publish an endpoint")
    }
    async fn control(&mut self, value: Value) {
        self.stdin.write_all(format!("{value}\n").as_bytes()).await.unwrap();
        assert_eq!(self.stdout.next_line().await.unwrap().as_deref(), Some("ok"));
    }
    fn channel(&self) -> ExtensionChannel {
        ExtensionChannel::new(ExtensionChannelOptions::new(self.folder.path()))
    }
}
fn args(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().unwrap().clone()
}
async fn await_events(events: &Rc<RefCell<Vec<LiveEvent>>>, count: usize) {
    for _ in 0..100 {
        if events.borrow().len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("missing extension event");
}
#[tokio::test]
async fn native_channel_interoperates_with_production_extension() {
    LocalSet::new().run_until(async{
 let mut peer=Peer::start().await;let channel=peer.channel();assert!(channel.connect().await.unwrap());let status=channel.status().unwrap();assert_eq!(status.adapter,LiveAdapterKind::Extension);assert!(status.has_operation("render.offline"));assert!(status.extra["extension"]["version"].is_string());
 let clip=channel.invoke("arrangement.midi-clip.create",&args(json!({"trackRef":"3:track:0","start":0,"length":4,"notes":[{"pitch":60,"start":0,"duration":1}],"expectedName":"Keys"})),None).await.unwrap();assert_eq!(clip["ref"],"3:arrangement_clip:0:0");assert_eq!(clip["notes"],1);
 let rendered=channel.invoke("render.offline",&args(json!({"trackRef":"3:track:2","fromBeat":0,"toBeat":4,"expectedName":"Vox"})),None).await.unwrap();assert_eq!(rendered["format"],"wav");assert_eq!(rendered["seconds"],2);
 assert!(channel.invoke("tempo.set",&args(json!({})),None).await.unwrap_err().message().contains("doesn't offer tempo.set"));assert!(channel.invoke("render.offline",&args(json!({"trackRef":"3:track:2"})),None).await.unwrap_err().message().contains("required by registry"));
 let events=Rc::new(RefCell::new(vec![]));let captured=events.clone();let listener:LiveListener=Rc::new(move|e|captured.borrow_mut().push(e.clone()));let off=channel.subscribe(listener.clone());let _duplicate=channel.subscribe(listener);peer.control(json!({"point":"vox"})).await;await_events(&events,1).await;assert_eq!(events.borrow().len(),1);assert_eq!(events.borrow()[0].event_type,LiveEventType::Pointed);assert_eq!(events.borrow()[0].payload["path"],json!([2]));off();peer.control(json!({"point":"keys"})).await;tokio::time::sleep(Duration::from_millis(20)).await;assert_eq!(events.borrow().len(),1);channel.close().await.unwrap();assert!(channel.status().is_none());
}).await;
}
#[tokio::test]
async fn cancelled_extension_render_leaves_channel_usable_and_late_answer_ignored() {
    LocalSet::new()
        .run_until(async {
            let mut peer = Peer::start().await;
            let channel = peer.channel();
            assert!(channel.connect().await.unwrap());
            peer.control(json!({"delay":300})).await;
            let signal = Signal::new();
            let abort = signal.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(Duration::from_millis(30)).await;
                abort.cancel();
            });
            let context = LiveOperationContext { signal: Some(signal), ..Default::default() };
            let render = args(json!({"trackRef":"3:track:2","fromBeat":0,"toBeat":4,"expectedName":"Vox"}));
            let error = channel.invoke("render.offline", &render, Some(&context)).await.unwrap_err();
            assert_eq!(error.message(), "Kumi stopped waiting for render.offline after sending it; Live may still finish it");
            assert!(channel.status().is_some());
            peer.control(json!({"delay":0})).await;
            assert_eq!(channel.invoke("render.offline", &render, None).await.unwrap()["format"], "wav");
            tokio::time::sleep(Duration::from_millis(350)).await;
            assert!(channel.status().is_some());
            assert_eq!(channel.invoke("render.offline", &render, None).await.unwrap()["seconds"], 2);
            channel.close().await.unwrap();
        })
        .await;
}
#[cfg(unix)]
#[test]
fn an_endpoint_naming_another_users_process_isnt_kumis() {
    if unsafe { libc::getuid() } == 0 {
        return;
    }
    // pid 1 runs as root: alive, but not this user's.
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("endpoint.json"), json!({"host":"127.0.0.1","port":1,"pid":1}).to_string()).unwrap();
    assert!(read_extension_endpoint(folder.path()).is_none());
    std::fs::write(folder.path().join("endpoint.json"), json!({"host":"127.0.0.1","port":1,"pid":std::process::id()}).to_string()).unwrap();
    assert!(read_extension_endpoint(folder.path()).is_some());
}
#[tokio::test]
async fn extension_discovery_checks_liveness_registry_secret_and_enabled_state() {
    LocalSet::new()
        .run_until(async {
            let peer = Peer::start().await;
            let endpoint = read_extension_endpoint(peer.folder.path()).unwrap();
            assert_eq!(endpoint["pid"].as_u64(), peer._child.id().map(u64::from));
            let copy = tempfile::tempdir().unwrap();
            std::fs::write(copy.path().join("endpoint.json"), serde_json::to_vec(&endpoint).unwrap()).unwrap();
            let channel = ExtensionChannel::new(ExtensionChannelOptions::new(copy.path()));
            assert!(!channel.connect().await.unwrap());
            assert_eq!(channel.reason(), "the extension's secret is missing");
            std::fs::write(copy.path().join("secret"), "short").unwrap();
            assert!(!channel.connect().await.unwrap());
            assert_eq!(channel.reason(), "the extension's secret is too short");
            std::fs::write(copy.path().join("secret"), "w".repeat(40)).unwrap();
            assert!(!channel.connect().await.unwrap());
            assert!(channel.reason().contains("signed with the bridge's secret"));
            let mut invalid = endpoint.clone();
            invalid["registryHash"] = "0".repeat(64).into();
            std::fs::write(copy.path().join("endpoint.json"), serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(!channel.connect().await.unwrap());
            assert!(channel.reason().contains("another bridge version"));
            invalid["pid"] = ((1u64 << 22) + 12345).into();
            std::fs::write(copy.path().join("endpoint.json"), serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(read_extension_endpoint(copy.path()).is_none());
            assert!(!channel.connect().await.unwrap());
            assert!(channel.reason().contains("isn't running"));
            let enabled = Rc::new(Cell::new(false));
            let state = enabled.clone();
            let launched = Rc::new(Cell::new(0));
            let launches = launched.clone();
            let mut options = ExtensionChannelOptions::new(peer.folder.path());
            options.enabled = Some(Rc::new(move || state.get()));
            options.launch = Some(Rc::new(move || {
                launches.set(launches.get() + 1);
                Box::pin(async { Ok(()) })
            }));
            let channel = ExtensionChannel::new(options);
            assert!(!channel.connect().await.unwrap());
            assert_eq!(channel.reason(), "no real Live is connected");
            assert_eq!(launched.get(), 0);
            enabled.set(true);
            assert!(channel.connect().await.unwrap());
            channel.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn installed_storage_is_preferred_and_launcher_can_share_another_folder() {
    LocalSet::new()
        .run_until(async {
            let peer = Peer::start().await;
            let absent = tempfile::tempdir().unwrap();
            let launches = Rc::new(Cell::new(0));
            let observed = launches.clone();
            let mut options = ExtensionChannelOptions::new(absent.path());
            options.installed_storage = Some(peer.folder.path().into());
            options.launch = Some(Rc::new(move || {
                observed.set(observed.get() + 1);
                Box::pin(async { Ok(()) })
            }));
            let channel = ExtensionChannel::new(options);
            assert!(channel.connect().await.unwrap());
            assert_eq!(launches.get(), 0);
            channel.close().await.unwrap();
            let shared: Rc<RefCell<Option<ExtensionChannel>>> = Rc::new(RefCell::new(None));
            let target = shared.clone();
            let folder = peer.folder.path().to_path_buf();
            let mut options = ExtensionChannelOptions::new(absent.path());
            options.launch = Some(Rc::new(move || {
                target.borrow().as_ref().unwrap().share(folder.clone());
                Box::pin(async { Ok(()) })
            }));
            let channel = ExtensionChannel::new(options);
            *shared.borrow_mut() = Some(channel.clone());
            assert!(channel.connect().await.unwrap());
            channel.close().await.unwrap();
            shared.take();
        })
        .await;
}
fn remote_status() -> LiveStatus {
    serde_json::from_value(json!({"connected":true,"adapter":"remote-script","epoch":7,"protocol":"ableton-live/v1","capabilities":["session.read"],"registryHash":*LIVE_REGISTRY_HASH,"operations":["status","snapshot","tempo.set","device.duplicate"]})).unwrap()
}
#[test]
fn routing_merges_only_extension_additions_and_retains_status_evidence() {
    let remote = remote_status();
    let extension:LiveStatus=serde_json::from_value(json!({"connected":true,"adapter":"extension","epoch":1,"protocol":"ableton-live/v1","capabilities":[],"operations":["status","render.offline","device.duplicate"],"extension":{"version":"1.0.0"}})).unwrap();
    assert!(route_to_extension("render.offline", &remote, Some(&extension)));
    assert!(!route_to_extension("device.duplicate", &remote, Some(&extension)));
    assert!(!route_to_extension("render.offline", &remote, None));
    let merged = merged_status(&remote, Some(&extension), "connected");
    assert_eq!(merged.operations.unwrap(), vec!["status", "snapshot", "tempo.set", "device.duplicate", "render.offline"]);
    assert_eq!(merged.extra["channels"], json!({"extension":{"connected":true,"version":"1.0.0","operations":["render.offline"]}}));
    assert_eq!(merged_status(&remote, None, "missing").extra["channels"], json!({"extension":{"connected":false,"reason":"missing"}}));
    let mut disconnected = remote.clone();
    disconnected.connected = false;
    assert_eq!(merged_status(&disconnected, Some(&extension), "x").operations, remote.operations);
}
#[tokio::test]
async fn routed_adapter_delegates_native_methods_and_tags_event_channels() {
    LocalSet::new()
        .run_until(async {
            let mut peer = Peer::start().await;
            let channel = peer.channel();
            channel.connect().await.unwrap();
            let simulator = Rc::new(DeterministicLiveSimulator::new());
            let remote: Rc<dyn AsyncLiveAdapter> = simulator.clone();
            let closed = Rc::new(Cell::new(0));
            let closes = closed.clone();
            let adapter = routed_adapter(remote, channel.clone(), Some(Rc::new(move || closes.set(closes.get() + 1))));
            assert!(adapter.status().unwrap().extra["channels"]["extension"]["connected"] == true);
            // Simulator advertises all extension operations, so shared operations use its own contract.
            let status = adapter.status().unwrap();
            assert!(!route_to_extension("render.offline", &status, channel.status().as_ref()));
            assert_eq!(adapter.snapshot_async(None, None).await.unwrap(), simulator.snapshot_async(None, None).await.unwrap());
            let extension_only = routed_adapter(Rc::new(UnavailableLiveAdapter), channel.clone(), None);
            let rendered = extension_only
                .invoke_async(
                    &LiveInvocation::new("render.offline", json!({"trackRef":"3:track:2","fromBeat":0,"toBeat":2,"expectedName":"Vox"})),
                    None,
                )
                .await
                .unwrap();
            assert_eq!(rendered["seconds"], 1);
            let events = Rc::new(RefCell::new(vec![]));
            let seen = events.clone();
            let off = adapter.subscribe(Rc::new(move |e| seen.borrow_mut().push(e.clone()))).unwrap();
            simulator.reconnect().unwrap();
            peer.control(json!({"point":"keys"})).await;
            await_events(&events, 2).await;
            assert_eq!(
                events.borrow().iter().map(|e| e.channel).collect::<Vec<_>>(),
                vec![Some(LiveEventChannel::RemoteScript), Some(LiveEventChannel::Extension)]
            );
            off();
            adapter.close().await.unwrap();
            assert_eq!(closed.get(), 1);
            assert!(channel.status().is_none());
        })
        .await;
}
#[test]
fn windows_folders_use_local_app_data() {
    let env = HashMap::from([
        ("LOCALAPPDATA".into(), "C:/Users/p/AppData/Local".into()),
        ("APPDATA".into(), "C:/Users/p/AppData/Roaming".into()),
    ]);
    let folders = kumi_extension_folders(Some(&env), Some("win32"), Some(Path::new("C:/Users/p"))).unwrap();
    assert_eq!(folders.data, PathBuf::from("C:/Users/p/AppData/Local/Ableton/Extensions Data/kumi.kumi"));
}
