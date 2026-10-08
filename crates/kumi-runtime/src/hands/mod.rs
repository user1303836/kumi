pub mod mac;
mod paths;
pub mod windows;
use crate::system;
use async_trait::async_trait;
use kumi_common::{abort::Signal, js::json::stringify};
pub use mac::HANDS_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuItem {
    pub path: Vec<String>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<f64>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HandsReply {
    #[serde(default)]
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<f64>,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HandsErrorKind {
    Untrusted,
    NoLive,
    Unavailable,
    #[default]
    Failed,
}
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct HandsError {
    pub message: String,
    pub kind: HandsErrorKind,
}
impl HandsError {
    pub fn new(message: impl Into<String>, kind: HandsErrorKind) -> Self {
        Self { message: message.into(), kind }
    }
    fn failed(message: impl Into<String>) -> Self {
        Self::new(message, HandsErrorKind::Failed)
    }
}
impl From<std::io::Error> for HandsError {
    fn from(e: std::io::Error) -> Self {
        Self::failed(e.to_string())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nth: Option<f64>,
}
#[derive(Clone, Default)]
pub struct MenuOptions {
    pub front: Option<bool>,
    pub signal: Option<Signal>,
    pub titles: Vec<String>,
}
#[derive(Clone, Default)]
pub struct KeysOptions {
    pub signal: Option<Signal>,
    pub gap_ms: Option<f64>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Dialog {
    pub open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buttons: Option<Vec<String>>,
    /// "save" or "open" for Windows' Save and Open dialogs, which Kumi can fill (#189).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Its toggles (check boxes), when it has any: Live's Separate Stems asks which stems with them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggles: Option<Vec<Toggle>>,
}
/// A dialog's toggle as Kumi's hands read it: its accessible name (Live's say what they do after a comma:
/// "Vocals, Include or exclude the Vocals stem."), whether it's on, whether it can be changed now (Separate
/// Stems' Merge Stems only with two or three stems on), and its accessibility id when it has one (Live's are
/// the same in every language: VocalsCheckControl).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Toggle {
    pub name: String,
    pub on: bool,
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}
fn yes() -> bool {
    true
}
fn is_yes(value: &bool) -> bool {
    *value
}
/// A toggle to set, by its id when it has one (else its name, whole, as `dialog` read it): on or off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToggleSet {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub on: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub title: String,
    pub subrole: String,
}
#[async_trait(?Send)]
pub trait Hands {
    async fn trusted(&self, prompt: bool) -> Result<bool, HandsError>;
    async fn menus(&self, signal: Option<Signal>) -> Result<Vec<MenuItem>, HandsError>;
    async fn tracks(&self, tracks: &[Track], signal: Option<Signal>) -> Result<HandsReply, HandsError>;
    async fn menu(&self, path: &[String], options: MenuOptions) -> Result<HandsReply, HandsError>;
    async fn keys(&self, combos: &[String], options: KeysOptions) -> Result<HandsReply, HandsError>;
    async fn dialog(&self, signal: Option<Signal>) -> Result<Dialog, HandsError>;
    async fn answer(&self, button: &str, signal: Option<Signal>) -> Result<HandsReply, HandsError>;
    /// Set the toggles of the dialog Live has open, in order, each on or off; one Live won't change yet is
    /// tried again after the rest. The reply has the toggles as they are after, the ones it couldn't find
    /// (`missing`) and the ones Live wouldn't change (`refused`).
    async fn toggles(&self, set: &[ToggleSet], signal: Option<Signal>) -> Result<HandsReply, HandsError> {
        let _ = (set, signal);
        Err(HandsError::new("Kumi can't set the toggles of Live's dialogs here.", HandsErrorKind::Unavailable))
    }
    async fn windows(&self, signal: Option<Signal>) -> Result<Vec<Window>, HandsError>;
    /// Fill the dialog Live has open with `path` and press its default button, only when it's the `kind`
    /// ("save" or "open") of dialog asked for (Windows).
    async fn file(&self, path: &str, kind: &str, signal: Option<Signal>) -> Result<HandsReply, HandsError> {
        let _ = (path, kind, signal);
        Err(HandsError::new("Kumi can't fill Live's file dialogs here.", HandsErrorKind::Unavailable))
    }
    fn close(&self);
}
pub type OnBuild = Rc<dyn Fn(&str)>;
#[derive(Clone, Default)]
pub struct MacHelperOptions {
    pub on_build: Option<OnBuild>,
    pub build: Option<bool>,
}
#[derive(Clone, Default)]
pub struct OpenHandsOptions {
    pub on_build: Option<OnBuild>,
    pub timeout_ms: Option<u64>,
    pub build: Option<bool>,
}
fn tools_dir() -> PathBuf {
    std::env::var("KUMI_TOOLS_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        std::env::var("KUMI_HOME").map(PathBuf::from).unwrap_or_else(|_| home::home_dir().unwrap_or_default().join(".kumi")).join("tools")
    })
}
fn digest(source: &str) -> String {
    hex::encode(Sha256::digest(source.as_bytes()))[..12].into()
}
fn compiler_present() -> bool {
    std::process::Command::new("xcrun")
        .args(["--find", "swiftc"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
pub fn can_build_hands() -> bool {
    system::platform() == "darwin" && compiler_present()
}
/// Keep the carried helper at its legacy path; the cache uses the same source hash.
pub async fn mac_helper(options: MacHelperOptions) -> Result<Option<String>, HandsError> {
    if let Ok(path) = std::env::var("KUMI_HANDS") {
        if !path.is_empty() {
            return Ok(Some(path));
        }
    }
    let hash = digest(mac::MAC_SOURCE);
    let name = format!("kumi-hands-{hash}");
    let carried = std::env::current_exe().ok().and_then(|p| paths::carried_helper(&p, &name));
    if let Some(carried) = carried.filter(|p| p.exists()) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&carried).is_ok_and(|m| m.permissions().mode() & 0o111 == 0) {
                let _ = std::fs::set_permissions(&carried, std::fs::Permissions::from_mode(0o755));
            }
        }
        return Ok(Some(carried.to_string_lossy().into()));
    }
    let folder = tools_dir().join("hands");
    let built = folder.join(&name);
    if built.exists() {
        return Ok(Some(built.to_string_lossy().into()));
    }
    if options.build == Some(false) || !compiler_present() {
        return Ok(None);
    }
    if let Some(notify) = options.on_build {
        notify("Getting Kumi's hands ready, once (a few seconds)…");
    }
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(&folder).await?;
    let source = folder.join(format!("KumiHands-{hash}.swift"));
    tokio::fs::write(&source, mac::MAC_SOURCE).await?;
    let temporary = PathBuf::from(format!("{}.{}", built.to_string_lossy(), std::process::id()));
    let compiled = tokio::process::Command::new("xcrun")
        .args(["swiftc", "-O", "-o"])
        .arg(&temporary)
        .arg(&source)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success());
    remove(&source).await?;
    if !compiled || !temporary.exists() {
        remove(&temporary).await?;
        return Ok(None);
    }
    tokio::fs::rename(temporary, &built).await?;
    Ok(Some(built.to_string_lossy().into()))
}
async fn remove(path: &Path) -> Result<(), std::io::Error> {
    match tokio::fs::remove_file(path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}
/// The Windows helper's script in `folder`, named by its hash. It's written whole, a temporary file renamed into
/// place, and written again when what's there isn't it (a write cut short before this one).
pub async fn windows_script(folder: &Path) -> Result<PathBuf, std::io::Error> {
    let script = folder.join(format!("kumi-hands-{}.ps1", digest(windows::WINDOWS_SOURCE)));
    if tokio::fs::read(&script).await.is_ok_and(|text| text == windows::WINDOWS_SOURCE.as_bytes()) {
        return Ok(script);
    }
    tokio::fs::create_dir_all(folder).await?;
    let temporary = folder.join(format!(".kumi-hands-{}.ps1", uuid::Uuid::new_v4()));
    let written = async {
        tokio::fs::write(&temporary, windows::WINDOWS_SOURCE).await?;
        tokio::fs::rename(&temporary, &script).await
    }
    .await;
    if let Err(error) = written {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    Ok(script)
}
pub async fn open_hands(options: OpenHandsOptions) -> Result<Option<Rc<dyn Hands>>, HandsError> {
    match system::platform() {
        "darwin" => Ok(mac_helper(MacHelperOptions { on_build: options.on_build, build: options.build })
            .await?
            .map(|helper| persistent(helper, Vec::new(), options.timeout_ms))),
        "win32" => {
            let script = windows_script(&tools_dir().join("hands")).await?;
            Ok(Some(persistent(
                "powershell.exe".into(),
                ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script.to_str().unwrap()]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                Some(options.timeout_ms.unwrap_or(8000)),
            )))
        }
        _ => Ok(None),
    }
}
type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<HandsReply>>>>;
enum Request {
    Ask(String),
    Close,
}
struct Persistent {
    send: mpsc::UnboundedSender<Request>,
    waiting: Waiting,
    next: AtomicU64,
    timeout_ms: u64,
}
fn settle(waiting: &Waiting, error: &str) {
    for (_, answer) in waiting.lock().unwrap().drain() {
        let _ = answer.send(HandsReply { ok: false, error: Some(error.into()), ..Default::default() });
    }
}
/// Start the helper on its first request, reuse it, and restart it after an exit.
pub fn persistent(command: String, args: Vec<String>, timeout_ms: Option<u64>) -> Rc<dyn Hands> {
    let (send, mut requests) = mpsc::unbounded_channel();
    let waiting: Waiting = Arc::new(Mutex::new(HashMap::new()));
    let responses = waiting.clone();
    tokio::spawn(async move {
        let mut child: Option<tokio::process::Child> = None;
        let mut input: Option<tokio::process::ChildStdin> = None;
        let mut out: Option<BufReader<tokio::process::ChildStdout>> = None;
        // The answer being read: read_until keeps what it has read in it when another branch goes first.
        let mut line: Vec<u8> = Vec::new();
        let mut errors: Option<tokio::process::ChildStderr> = None;
        let mut discard = [0u8; 8192];
        loop {
            tokio::select! {
                request=requests.recv()=>match request{
                    Some(Request::Ask(text))=>{
                        if child.is_none(){
                            let mut launch=tokio::process::Command::new(&command);launch.args(&args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);#[cfg(windows)]launch.creation_flags(0x0800_0000);
                            match launch.spawn(){Ok(mut started)=>{input=started.stdin.take();out=started.stdout.take().map(BufReader::new);line.clear();errors=started.stderr.take();child=Some(started);},Err(_)=>{settle(&responses,"helper-failed");continue;}}
                        }
                        if let Some(stdin)=&mut input{if stdin.write_all(text.as_bytes()).await.is_err(){settle(&responses,"helper-failed");}}
                    },
                    Some(Request::Close)=>{input=None;out=None;line.clear();errors=None;if let Some(mut process)=child.take(){#[cfg(unix)]if let Some(pid)=process.id(){unsafe{libc::kill(pid as i32,libc::SIGTERM);}}#[cfg(not(unix))]let _=process.start_kill();tokio::spawn(async move{let _=process.wait().await;});}settle(&responses,"helper-ended");},
                    None=>{if let Some(mut process)=child.take(){let _=process.start_kill();let _=process.wait().await;}settle(&responses,"helper-ended");break;}
                },
                // Read whatever a console's code page made of a line: one that isn't UTF-8 isn't an answer, and the next still is.
                read=async{match &mut out{Some(out)=>out.read_until(b'\n',&mut line).await,None=>std::future::pending().await}}=>match read{Ok(read)if read>0=>{if let Ok(value)=serde_json::from_str::<Value>(&String::from_utf8_lossy(&line)){if let Some(id)=value.get("id").and_then(Value::as_f64).filter(|v|v.is_finite()&&*v>=0.0&&v.fract()==0.0){if let Ok(reply)=serde_json::from_value::<HandsReply>(value){if let Some(answer)=responses.lock().unwrap().remove(&(id as u64)){let _=answer.send(reply);}}}}line.clear();},_=>{out=None;line.clear();}},
                read=async{match &mut errors{Some(stderr)=>stderr.read(&mut discard).await,None=>std::future::pending().await}}=>if !matches!(read,Ok(n)if n>0){errors=None;},
                status=async{match &mut child{Some(child)=>child.wait().await,None=>std::future::pending().await}}=>{settle(&responses,if status.is_ok(){"helper-ended"}else{"helper-failed"});child=None;input=None;out=None;line.clear();errors=None;}
            }
        }
    });
    Rc::new(Persistent { send, waiting, next: AtomicU64::new(1), timeout_ms: timeout_ms.unwrap_or(4000) })
}
/// JSON with each character past ASCII as a \u escape: Windows PowerShell reads its input in the console's code page.
fn ascii(json: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                let _ = write!(out, "\\u{unit:04x}");
            }
        }
    }
    out
}
struct Pending {
    waiting: Waiting,
    id: u64,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.waiting.lock().unwrap().remove(&self.id);
    }
}
impl Persistent {
    async fn ask(&self, op: &str, fields: Value, signal: Option<Signal>) -> Result<HandsReply, HandsError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (answer, receive) = oneshot::channel();
        self.waiting.lock().unwrap().insert(id, answer);
        let _pending = Pending { waiting: self.waiting.clone(), id };
        let mut request = json!({"id":id,"op":op});
        if let Some(fields) = fields.as_object() {
            request.as_object_mut().unwrap().extend(fields.clone());
        }
        self.send
            .send(Request::Ask(format!("{}\n", ascii(&stringify(&request)))))
            .map_err(|_| HandsError::new("Kumi's hands couldn't start.", HandsErrorKind::Unavailable))?;
        tokio::select! {reply=receive=>reply.map_err(|_|HandsError::new("Kumi's hands couldn't start.",HandsErrorKind::Unavailable)),_=tokio::time::sleep(Duration::from_millis(self.timeout_ms))=>Err(HandsError::failed("Live didn't answer in time; is a dialog open in Live?")),_=async{if let Some(signal)=signal{signal.cancelled().await;}else{std::future::pending::<()>().await;}}=>Err(HandsError::failed("Stopped"))}
    }
    fn checked(reply: HandsReply) -> Result<HandsReply, HandsError> {
        if reply.ok {
            return Ok(reply);
        }
        match reply.error.as_deref() {
            Some("untrusted") => Err(HandsError::new(
                if system::platform() == "darwin" {
                    "Kumi needs Accessibility access to use Live's menus: System Settings › Privacy & Security › Accessibility, then turn on the app Kumi runs in (your terminal). Then ask again."
                } else {
                    "Windows refused Kumi's access to Live's window."
                },
                HandsErrorKind::Untrusted,
            )),
            Some("no-live") => Err(HandsError::new("Live isn't running.", HandsErrorKind::NoLive)),
            _ => Ok(reply),
        }
    }
}
#[async_trait(?Send)]
impl Hands for Persistent {
    async fn trusted(&self, prompt: bool) -> Result<bool, HandsError> {
        Ok(self.ask("trusted", json!({"prompt":prompt}), None).await?.fields.get("trusted") == Some(&Value::Bool(true)))
    }
    async fn menus(&self, signal: Option<Signal>) -> Result<Vec<MenuItem>, HandsError> {
        let reply = Self::checked(self.ask("menus", json!({}), signal).await?)?;
        Ok(reply.fields.get("items").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default())
    }
    async fn tracks(&self, tracks: &[Track], signal: Option<Signal>) -> Result<HandsReply, HandsError> {
        Self::checked(self.ask("tracks", json!({"tracks":tracks}), signal).await?)
    }
    async fn menu(&self, path: &[String], options: MenuOptions) -> Result<HandsReply, HandsError> {
        let mut fields = json!({"path":path,"front":options.front.unwrap_or(false)});
        if !options.titles.is_empty() {
            fields["titles"] = json!(options.titles);
        }
        Self::checked(self.ask("menu", fields, options.signal).await?)
    }
    async fn keys(&self, combos: &[String], options: KeysOptions) -> Result<HandsReply, HandsError> {
        let mut fields = json!({"keys":combos});
        if let Some(gap) = options.gap_ms {
            fields["gapMs"] = json!(gap);
        }
        Self::checked(self.ask("keys", fields, options.signal).await?)
    }
    async fn dialog(&self, signal: Option<Signal>) -> Result<Dialog, HandsError> {
        let reply = Self::checked(self.ask("dialog", json!({}), signal).await?)?;
        Ok(Dialog {
            open: reply.fields.get("open") == Some(&Value::Bool(true)),
            title: reply.fields.get("title").and_then(Value::as_str).map(str::to_string),
            words: reply.fields.get("words").and_then(|v| serde_json::from_value(v.clone()).ok()),
            buttons: reply.fields.get("buttons").and_then(|v| serde_json::from_value(v.clone()).ok()),
            file: reply.fields.get("file").and_then(Value::as_str).map(str::to_string),
            toggles: reply.fields.get("toggles").and_then(|v| serde_json::from_value(v.clone()).ok()),
        })
    }
    async fn answer(&self, button: &str, signal: Option<Signal>) -> Result<HandsReply, HandsError> {
        Self::checked(self.ask("answer", json!({"button":button}), signal).await?)
    }
    async fn toggles(&self, set: &[ToggleSet], signal: Option<Signal>) -> Result<HandsReply, HandsError> {
        Self::checked(self.ask("toggles", json!({"set":set}), signal).await?)
    }
    async fn file(&self, path: &str, kind: &str, signal: Option<Signal>) -> Result<HandsReply, HandsError> {
        Self::checked(self.ask("file", json!({"path":path,"kind":kind}), signal).await?)
    }
    async fn windows(&self, signal: Option<Signal>) -> Result<Vec<Window>, HandsError> {
        let reply = Self::checked(self.ask("windows", json!({}), signal).await?)?;
        Ok(reply.fields.get("windows").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default())
    }
    fn close(&self) {
        let _ = self.send.send(Request::Close);
    }
}
