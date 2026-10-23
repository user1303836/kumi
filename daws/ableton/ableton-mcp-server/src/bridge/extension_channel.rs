//! Kumi's Live extension channel: authenticated requests and events, without mutation preflight.
use super::listeners::Listeners;
use super::wire;
use crate::{
    live::*,
    loopback::LOOPBACK_PROTOCOL_VERSION,
    registry::{validate_live_operation_request, validate_live_operation_result},
};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_common::{abort::Signal, time::now_ms};
use serde_json::{json, Map, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{tcp::OwnedWriteHalf, TcpStream},
    sync::{oneshot, Mutex},
};
/// Endpoint metadata is retained exactly as the extension wrote it. Only host, port and pid are validated on discovery.
pub type ExtensionEndpoint = Value;
pub type ExtensionLaunch = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<(), LiveError>>>;
#[derive(Clone)]
pub struct ExtensionChannelOptions {
    pub storage_directory: PathBuf,
    pub installed_storage: Option<PathBuf>,
    pub launch: Option<ExtensionLaunch>,
    pub enabled: Option<Rc<dyn Fn() -> bool>>,
    pub timeout_ms: Option<f64>,
    pub log: Option<Rc<dyn Fn(&str)>>,
}
impl ExtensionChannelOptions {
    pub fn new(storage_directory: impl Into<PathBuf>) -> Self {
        Self {
            storage_directory: storage_directory.into(),
            installed_storage: None,
            launch: None,
            enabled: None,
            timeout_ms: None,
            log: None,
        }
    }
}
pub fn read_extension_endpoint(storage: &Path) -> Option<ExtensionEndpoint> {
    // Only this user's folder and endpoint: another user's are never Kumi's to connect to.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let owned = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.uid() == uid);
        if !owned(storage) || !owned(&storage.join("endpoint.json")) {
            return None;
        }
    }
    let value: Value = serde_json::from_slice(&std::fs::read(storage.join("endpoint.json")).ok()?).ok()?;
    let port = value["port"].as_f64()?;
    let pid = value["pid"].as_f64()?;
    if value["host"] != "127.0.0.1"
        || !port.is_finite()
        || port.fract() != 0.0
        || !pid.is_finite()
        || pid.fract() != 0.0
        || pid < i32::MIN as f64
        || pid > i32::MAX as f64
        || !process_alive(pid as i32)
    {
        return None;
    }
    Some(value)
}
/// Whether `pid` runs, as this user (another user's process, EPERM, isn't one Kumi's endpoint can be).
pub(super) fn process_alive(pid: i32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid, 0) == 0 }
    }
    #[cfg(windows)]
    {
        unsafe {
            #[link(name = "kernel32")]
            extern "system" {
                fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
                fn GetExitCodeProcess(handle: *mut std::ffi::c_void, code: *mut u32) -> i32;
                fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
                fn GetLastError() -> u32;
            }
            let handle = OpenProcess(0x1000, 0, pid as u32);
            if handle.is_null() {
                return GetLastError() == 5;
            }
            let mut code = 0;
            let success = GetExitCodeProcess(handle, &mut code) != 0;
            CloseHandle(handle);
            success && code == 259
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}
struct Pending {
    operation: String,
    response: oneshot::Sender<Result<Value, LiveError>>,
}
struct ExtensionInner {
    options: ExtensionChannelOptions,
    storage: RefCell<PathBuf>,
    writer: RefCell<Option<Rc<Mutex<OwnedWriteHalf>>>>,
    cancel: RefCell<Signal>,
    generation: Cell<u64>,
    hello: RefCell<Option<(String, String)>>,
    greeting: RefCell<Option<oneshot::Sender<Result<(), LiveError>>>>,
    secret: RefCell<Option<String>>,
    cached: RefCell<Option<LiveStatus>>,
    sequence: Cell<u64>,
    connecting: RefCell<Option<Shared<LocalBoxFuture<'static, Result<bool, LiveError>>>>>,
    pending: RefCell<HashMap<String, Pending>>,
    listeners: Listeners<dyn Fn(&LiveEvent)>,
    status_listeners: Listeners<dyn Fn(Option<&LiveStatus>)>,
    endpoint: RefCell<Option<ExtensionEndpoint>>,
    reason: RefCell<String>,
}
#[derive(Clone)]
pub struct ExtensionChannel(Rc<ExtensionInner>);
impl ExtensionChannel {
    pub fn new(options: ExtensionChannelOptions) -> Self {
        let storage = options.storage_directory.clone();
        Self(Rc::new(ExtensionInner {
            options,
            storage: RefCell::new(storage),
            writer: RefCell::new(None),
            cancel: RefCell::new(Signal::new()),
            generation: Cell::new(0),
            hello: RefCell::new(None),
            greeting: RefCell::new(None),
            secret: RefCell::new(None),
            cached: RefCell::new(None),
            sequence: Cell::new(0),
            connecting: RefCell::new(None),
            pending: RefCell::new(HashMap::new()),
            listeners: Listeners::default(),
            status_listeners: Listeners::default(),
            endpoint: RefCell::new(None),
            reason: RefCell::new("not connected yet".into()),
        }))
    }
    pub fn status(&self) -> Option<LiveStatus> {
        if self.0.writer.borrow().is_some() && !self.0.cancel.borrow().is_cancelled() {
            self.0.cached.borrow().clone()
        } else {
            None
        }
    }
    pub fn reason(&self) -> String {
        self.0.reason.borrow().clone()
    }
    pub fn endpoint(&self) -> Option<ExtensionEndpoint> {
        self.0.endpoint.borrow().clone()
    }
    pub(super) fn share_callback(&self) -> Rc<dyn Fn(&Path)> {
        let weak = Rc::downgrade(&self.0);
        Rc::new(move |folder| {
            if let Some(inner) = weak.upgrade() {
                *inner.storage.borrow_mut() = folder.into();
            }
        })
    }
    pub fn share(&self, folder: impl Into<PathBuf>) {
        *self.0.storage.borrow_mut() = folder.into();
    }
    pub fn subscribe(&self, listener: LiveListener) -> Unsubscribe {
        self.0.listeners.add(listener.clone());
        let weak = Rc::downgrade(&self.0);
        Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.listeners.remove(&listener);
            }
        })
    }
    pub fn subscribe_status(&self, listener: StatusListener) -> Unsubscribe {
        self.0.status_listeners.add(listener.clone());
        let weak = Rc::downgrade(&self.0);
        Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.status_listeners.remove(&listener);
            }
        })
    }
    fn emit_status(&self, status: Option<&LiveStatus>) {
        self.0.status_listeners.each(|listener| listener(status));
    }
    pub async fn connect(&self) -> Result<bool, LiveError> {
        if self.status().is_some() {
            return Ok(true);
        }
        if self.0.options.enabled.as_ref().is_some_and(|enabled| !enabled()) {
            *self.0.reason.borrow_mut() = "no real Live is connected".into();
            return Ok(false);
        }
        let connecting = self.0.connecting.borrow().clone();
        if let Some(connecting) = connecting {
            return connecting.await;
        }
        let this = self.clone();
        let connecting = async move {
            let result = this.open().await;
            this.0.connecting.take();
            result
        }
        .boxed_local()
        .shared();
        *self.0.connecting.borrow_mut() = Some(connecting.clone());
        let background = connecting.clone();
        tokio::task::spawn_local(async move {
            let _ = background.await;
        });
        connecting.await
    }
    fn find(&self) -> Option<(PathBuf, ExtensionEndpoint)> {
        let mut seen = HashSet::new();
        for folder in [
            self.0.options.installed_storage.clone(),
            Some(self.0.storage.borrow().clone()),
            Some(self.0.options.storage_directory.clone()),
        ]
        .into_iter()
        .flatten()
        {
            if folder.as_os_str().is_empty() || !seen.insert(folder.clone()) {
                continue;
            }
            if let Some(endpoint) = read_extension_endpoint(&folder) {
                return Some((folder, endpoint));
            }
        }
        None
    }
    async fn open(&self) -> Result<bool, LiveError> {
        let mut found = self.find();
        if found.is_none() {
            if let Some(launch) = &self.0.options.launch {
                launch().await?;
                found = self.find();
            }
        }
        let Some((storage, endpoint)) = found else {
            *self.0.reason.borrow_mut() = "Kumi's Live extension isn't running".into();
            return Ok(false);
        };
        *self.0.storage.borrow_mut() = storage.clone();
        if endpoint["registryHash"] != *LIVE_REGISTRY_HASH {
            *self.0.reason.borrow_mut() = "Kumi's Live extension is from another bridge version".into();
            return Ok(false);
        }
        let secret = match std::fs::read(storage.join("secret")) {
            Ok(bytes) => kumi_common::js::string::trim(&String::from_utf8_lossy(&bytes)).to_owned(),
            Err(_) => {
                *self.0.reason.borrow_mut() = "the extension's secret is missing".into();
                return Ok(false);
            }
        };
        if kumi_common::js::string::utf16_len(&secret) < 32 {
            *self.0.reason.borrow_mut() = "the extension's secret is too short".into();
            return Ok(false);
        }
        *self.0.secret.borrow_mut() = Some(secret);
        *self.0.endpoint.borrow_mut() = Some(endpoint.clone());
        let result = async {
            self.handshake(&endpoint).await?;
            let status = self.request(json!({"method":"status"}), "status", None).await?;
            if status["adapter"] != "extension" || status["registryHash"] != *LIVE_REGISTRY_HASH {
                return Err(LiveError::error("Kumi's Live extension answered for something else"));
            }
            let status: LiveStatus = serde_json::from_value(status)?;
            *self.0.cached.borrow_mut() = Some(status.clone());
            *self.0.reason.borrow_mut() = "connected".into();
            self.emit_status(Some(&status));
            if let Some(log) = &self.0.options.log {
                let version = endpoint.get("extensionVersion").map(js_string).unwrap_or_else(|| "undefined".into());
                log(&format!("extension channel: connected to Kumi's Live extension {version} on port {}", endpoint["port"]));
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            *self.0.reason.borrow_mut() = error.to_string();
            self.0.cancel.borrow().cancel();
            self.0.writer.take();
            return Ok(false);
        }
        Ok(true)
    }
    fn fail_pending(&self, error: LiveError) {
        for (_, pending) in self.0.pending.take() {
            let _ = pending.response.send(Err(error.clone()));
        }
    }
    fn disconnected(&self) {
        self.0.cancel.borrow().cancel();
        self.0.writer.take();
        *self.0.reason.borrow_mut() = "Kumi's Live extension closed the connection".into();
        self.fail_pending(LiveError::error("Kumi's Live extension closed the connection"));
        self.emit_status(None);
    }
    async fn handshake(&self, endpoint: &Value) -> Result<(), LiveError> {
        self.0.hello.take();
        self.0.sequence.set(0);
        let (send, recv) = oneshot::channel();
        *self.0.greeting.borrow_mut() = Some(send);
        let port = endpoint["port"].as_f64().unwrap_or(0.0);
        if !(0.0..=65535.0).contains(&port) {
            return Err(LiveError::error(format!("Port should be >= 0 and < 65536. Received type number ({port}).")));
        }
        let opening = async {
            let stream = TcpStream::connect(("127.0.0.1", port as u16)).await.map_err(|e| LiveError::error(e.to_string()))?;
            stream.set_nodelay(true).map_err(|e| LiveError::error(e.to_string()))?;
            let (mut reader, writer) = stream.into_split();
            self.0.cancel.borrow().cancel();
            let cancel = Signal::new();
            *self.0.cancel.borrow_mut() = cancel.clone();
            *self.0.writer.borrow_mut() = Some(Rc::new(Mutex::new(writer)));
            let generation = self.0.generation.get() + 1;
            self.0.generation.set(generation);
            let weak = Rc::downgrade(&self.0);
            tokio::task::spawn_local(async move {
                let mut buffer = bytes::BytesMut::new();
                // How far the buffer is known to hold no newline: each byte is searched once, so a large
                // frame arriving in 64 KiB reads isn't rescanned from its start after each one.
                let mut scanned = 0;
                let mut chunk = vec![0; 65536];
                loop {
                    let read = tokio::select! {biased;_=cancel.cancelled()=>break,read=reader.read(&mut chunk)=>read};
                    let Some(inner) = weak.upgrade() else { break };
                    let channel = Self(inner);
                    if channel.0.generation.get() != generation {
                        break;
                    }
                    let count = match read {
                        Ok(0) => {
                            if let Some(greeting) = channel.0.greeting.take() {
                                let _ =
                                    greeting.send(Err(LiveError::error("Kumi's Live extension closed the connection before its hello")));
                            }
                            channel.disconnected();
                            break;
                        }
                        Ok(n) => n,
                        Err(error) => {
                            if let Some(greeting) = channel.0.greeting.take() {
                                let _ = greeting.send(Err(LiveError::error(error.to_string())));
                            }
                            channel.disconnected();
                            break;
                        }
                    };
                    buffer.extend_from_slice(&chunk[..count]);
                    let mut failure = None;
                    if !chunk[..count].contains(&b'\n') && buffer.len() > wire::MAX_FRAME_BYTES {
                        failure = Some(LiveError::error("the extension sent a frame beyond the bound"));
                    }
                    while failure.is_none() {
                        let Some(found) = memchr::memchr(b'\n', &buffer[scanned..]) else {
                            scanned = buffer.len();
                            break;
                        };
                        let index = scanned + found;
                        scanned = 0;
                        let line = buffer.split_to(index + 1);
                        if index == 0 {
                            continue;
                        }
                        let result = wire::parse(&line[..index]).and_then(|frame| channel.on_data(frame));
                        if let Err(error) = result {
                            failure = Some(error);
                        }
                    }
                    if let Some(error) = failure {
                        if let Some(greeting) = channel.0.greeting.take() {
                            let _ = greeting.send(Err(error));
                        }
                        channel.disconnected();
                        break;
                    }
                }
            });
            recv.await.unwrap_or_else(|_| Err(LiveError::error("Kumi's Live extension closed the connection")))
        };
        match tokio::time::timeout(Duration::from_secs(5), opening).await {
            Ok(result) => result,
            Err(_) => {
                self.0.cancel.borrow().cancel();
                Err(LiveError::error("the extension didn't greet the bridge"))
            }
        }
    }
    fn on_data(&self, mut frame: Value) -> Result<(), LiveError> {
        if frame["version"] != LOOPBACK_PROTOCOL_VERSION
            || !wire::verify(self.0.secret.borrow().as_deref().unwrap_or_default(), &mut frame, true)?
        {
            return Err(LiveError::error("the extension's answer isn't signed with the bridge's secret"));
        }
        if frame["id"] == "hello" {
            if frame["result"]["registryHash"] != *LIVE_REGISTRY_HASH {
                return Err(LiveError::error("Kumi's Live extension is from another bridge version"));
            }
            *self.0.hello.borrow_mut() = Some((
                frame.get("bridgeEpoch").map(js_string).unwrap_or_else(|| "undefined".into()),
                frame.get("connectionChallenge").map(js_string).unwrap_or_else(|| "undefined".into()),
            ));
            if let Some(greeting) = self.0.greeting.take() {
                let _ = greeting.send(Ok(()));
            }
            return Ok(());
        }
        if !self.0.hello.borrow().as_ref().is_some_and(|(epoch, challenge)| {
            frame["bridgeEpoch"].as_str() == Some(epoch) && frame["connectionChallenge"].as_str() == Some(challenge)
        }) {
            return Err(LiveError::error("the extension's answer belongs to another connection"));
        }
        if frame["id"] == "event" {
            if let Some(event) = frame.get("result").and_then(|v| v.get("event")).filter(|v| v["type"].is_string()) {
                let event: LiveEvent = serde_json::from_value(event.clone())?;
                self.0.listeners.each(|listener| listener(&event));
            }
            return Ok(());
        }
        let id = frame.get("id").map(js_string).unwrap_or_else(|| "undefined".into());
        let Some(pending) = self.0.pending.borrow_mut().remove(&id) else {
            return Ok(());
        };
        let result = if frame["ok"] == true {
            let result = frame["result"].clone();
            if pending.operation != "status" {
                validate_live_operation_result(&pending.operation, &result).map(|_| result).map_err(LiveError::from)
            } else {
                Ok(result)
            }
        } else {
            Err(LiveError::error(
                frame["error"]
                    .as_str()
                    .map(|error| format!("Kumi's Live extension: {error}"))
                    .unwrap_or_else(|| "Kumi's Live extension refused the request".into()),
            ))
        };
        let _ = pending.response.send(result);
        Ok(())
    }
    async fn request(&self, mut fields: Value, operation: &str, context: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        let writer = self.0.writer.borrow().clone();
        let hello = self.0.hello.borrow().clone();
        let (Some(writer), Some((epoch, challenge))) = (writer, hello) else {
            return Err(LiveError::error("Kumi's Live extension isn't connected"));
        };
        if self.0.cancel.borrow().is_cancelled() {
            return Err(LiveError::error("Kumi's Live extension isn't connected"));
        }
        if context.and_then(|c| c.signal.as_ref()).is_some_and(Signal::is_cancelled) {
            return Err(LiveError::error("request cancelled before dispatch"));
        }
        let timeout = context
            .and_then(|c| c.deadline_ms)
            .map(|deadline| (deadline - now_ms() as f64).max(1.0))
            .unwrap_or(self.0.options.timeout_ms.unwrap_or(30000.0));
        let sequence = self.0.sequence.get() + 1;
        self.0.sequence.set(sequence);
        let id = format!("ext-{sequence}");
        let row = fields.as_object_mut().expect("internal request fields");
        row.insert("version".into(), LOOPBACK_PROTOCOL_VERSION.into());
        row.insert("id".into(), id.clone().into());
        row.insert("nonce".into(), wire::random_id().into());
        row.insert("sequence".into(), sequence.into());
        row.insert("bridgeEpoch".into(), epoch.into());
        row.insert("connectionChallenge".into(), challenge.into());
        row.insert("deadlineMs".into(), json!(now_ms() as f64 + timeout.min(600000.0)));
        let frame = wire::signed(self.0.secret.borrow().as_deref().unwrap_or_default(), fields, true)?;
        let mut encoded = kumi_common::js::json::stringify(&frame).into_bytes();
        encoded.push(b'\n');
        let (send, mut recv) = oneshot::channel();
        self.0.pending.borrow_mut().insert(id.clone(), Pending { operation: operation.into(), response: send });
        let signal = context.and_then(|c| c.signal.clone()).unwrap_or_default();
        // 0 before the frame starts going out, 1 while it's written, 2 once it's all out.
        let written = Cell::new(0u8);
        let exchange = async {
            tokio::select! {result=async{let mut writer=writer.lock().await;written.set(1);let result=writer.write_all(&encoded).await;if result.is_ok(){written.set(2)}result}=>{result.map_err(|e|LiveError::error(e.to_string()))?;},result=&mut recv=>return result.unwrap_or_else(|_|Err(LiveError::error("the extension channel closed")))}
            recv.await.unwrap_or_else(|_| Err(LiveError::error("the extension channel closed")))
        };
        let timeout = if timeout.is_finite() && timeout >= 1.0 && timeout <= i32::MAX as f64 { timeout } else { 1.0 };
        let result = tokio::select! {result=exchange=>result,_=signal.cancelled()=>Err(LiveError::error(format!("Kumi stopped waiting for {operation} after sending it; Live may still finish it"))),_=tokio::time::sleep(Duration::from_secs_f64(timeout/1000.0))=>Err(LiveError::error(format!("Kumi's Live extension didn't answer {operation} in time")))};
        self.0.pending.borrow_mut().remove(&id);
        // Cut off partway through its frame: the rest would run into the next request's, so the connection goes.
        if written.get() == 1 {
            self.disconnected();
        }
        result
    }
    pub async fn invoke(
        &self,
        operation: &str,
        args: &Map<String, Value>,
        context: Option<&LiveOperationContext>,
    ) -> Result<Value, LiveError> {
        if !self.connect().await? {
            return Err(LiveError::error(format!("Kumi's Live extension is unavailable: {}", self.reason())));
        }
        if !self.0.cached.borrow().as_ref().is_some_and(|s| s.has_operation(operation)) {
            return Err(LiveError::error(format!("Kumi's Live extension doesn't offer {operation}")));
        }
        validate_live_operation_request(operation, &Value::Object(args.clone()))?;
        self.request(json!({"method":"invoke","operation":operation,"args":args}), operation, context).await
    }
    pub async fn close(&self) -> Result<(), LiveError> {
        self.0.writer.take();
        self.0.cancel.borrow().cancel();
        self.fail_pending(LiveError::error("the extension channel closed"));
        Ok(())
    }
}
fn js_string(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Null => "null".into(),
        Value::Object(_) => "[object Object]".into(),
        Value::Array(items) => items.iter().map(|v| if v.is_null() { String::new() } else { js_string(v) }).collect::<Vec<_>>().join(","),
        v => kumi_common::js::json::stringify(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    #[tokio::test(flavor = "current_thread")]
    async fn a_frame_cut_off_partway_closes_the_connection() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let secret = "w".repeat(40);
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let port = listener.local_addr().unwrap().port();
                // An extension that greets and answers status, then stops reading, keeping the connection.
                let served = secret.clone();
                tokio::task::spawn_local(async move {
                    let (socket, _) = listener.accept().await.unwrap();
                    let (reader, mut writer) = socket.into_split();
                    let mut lines = tokio::io::BufReader::new(reader).lines();
                    let (epoch, challenge) = ("epoch-0000000000000001", "challenge-000000000001");
                    let frame = |fields: Value| {
                        format!("{}\n", kumi_common::js::json::stringify(&wire::signed(&served, fields, true).unwrap())).into_bytes()
                    };
                    let hello = json!({"version":LOOPBACK_PROTOCOL_VERSION,"id":"hello","ok":true,"bridgeEpoch":epoch,"connectionChallenge":challenge,"result":{"protocol":"ableton-live/v1","registryHash":*LIVE_REGISTRY_HASH}});
                    writer.write_all(&frame(hello)).await.unwrap();
                    let request: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    let status = json!({"connected":true,"adapter":"extension","epoch":1,"protocol":"ableton-live/v1","capabilities":[],"registryHash":*LIVE_REGISTRY_HASH,"operations":["status"]});
                    let answer = json!({"version":LOOPBACK_PROTOCOL_VERSION,"id":request["id"],"ok":true,"bridgeEpoch":epoch,"connectionChallenge":challenge,"result":status});
                    writer.write_all(&frame(answer)).await.unwrap();
                    std::future::pending::<()>().await;
                    drop((lines, writer));
                });
                let folder = tempfile::tempdir().unwrap();
                let endpoint = json!({"host":"127.0.0.1","port":port,"pid":std::process::id(),"registryHash":*LIVE_REGISTRY_HASH});
                std::fs::write(folder.path().join("endpoint.json"), endpoint.to_string()).unwrap();
                std::fs::write(folder.path().join("secret"), &secret).unwrap();
                let mut options = ExtensionChannelOptions::new(folder.path());
                options.timeout_ms = Some(500.0);
                let channel = ExtensionChannel::new(options);
                assert!(channel.connect().await.unwrap(), "{}", channel.reason());
                // Far more than the socket's buffers hold: one is still going out when its request gives up.
                // Windows takes a whole send while its buffer is under quota, and one more past it, so it can take three.
                let request = json!({"method":"status","pad":"x".repeat(64 << 20)});
                for _ in 0..4 {
                    let error = channel.request(request.clone(), "status", None).await.unwrap_err();
                    assert!(error.to_string().contains("in time"), "{error}");
                    if channel.0.writer.borrow().is_none() {
                        break;
                    }
                }
                assert!(channel.0.writer.borrow().is_none(), "the connection with half a frame in it stayed open");
                assert!(channel.request(json!({"method":"status"}), "status", None).await.unwrap_err().to_string().contains("isn't connected"));
            })
            .await;
    }
}
