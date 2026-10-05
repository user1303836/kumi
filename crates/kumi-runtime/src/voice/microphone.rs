use crate::system::{self, Env};
use kumi_common::abort::Signal;
use kumi_common::js::{
    number::round,
    string::{head, slice, trim},
};
use regex::Regex;
use std::{
    process::Stdio,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::{io::AsyncReadExt, sync::Notify};
pub const RATE: usize = 16_000;
const BLOCK: usize = 512;
const BLOCK_MS: f64 = 32.0;
const MAX_BYTES: usize = RATE * 2 * 150;
pub fn microphone_input(platform: &str, device: Option<&str>) -> Vec<String> {
    let device = device.unwrap_or("");
    match platform {
        "darwin" => {
            vec!["-f".into(), "avfoundation".into(), "-i".into(), format!(":{}", if device.is_empty() { "default" } else { device })]
        }
        "win32" => vec!["-f".into(), "dshow".into(), "-audio_buffer_size".into(), "50".into(), "-i".into(), format!("audio={device}")],
        _ => ["-f", "pulse", "-i", if device.is_empty() { "default" } else { device }].into_iter().map(str::to_string).collect(),
    }
}
pub fn parse_microphones(listing: &str, platform: &str) -> Vec<String> {
    static PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[[^\]]*\]\s?").unwrap());
    static MAC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[([0-9]+)\] (.+)$").unwrap());
    static WINDOWS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\s*"(.+)"\s*(\((audio|video|none)\))?\s*$"#).unwrap());
    let mut names = Vec::new();
    let mut audio = false;
    for raw in listing.split('\n') {
        let line = PREFIX.replace(raw.trim_end_matches('\r'), "");
        let name = match platform {
            "darwin" => {
                if line.contains("AVFoundation audio devices:") {
                    audio = true;
                    continue;
                }
                if line.contains("AVFoundation video devices:") {
                    audio = false;
                    continue;
                }
                if audio {
                    MAC.captures(trim(&line)).map(|c| trim(&c[2]).to_string())
                } else {
                    None
                }
            }
            "win32" => {
                if line.contains("DirectShow audio devices") {
                    audio = true;
                    continue;
                }
                if line.contains("DirectShow video devices") {
                    audio = false;
                    continue;
                }
                if line.contains("Alternative name") {
                    continue;
                }
                WINDOWS
                    .captures(&line)
                    .filter(|c| c.get(3).is_some_and(|s| s.as_str() == "audio") || c.get(3).is_none() && audio)
                    .map(|c| c[1].to_string())
            }
            _ => None,
        };
        if let Some(name) = name {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}
pub async fn list_microphones(ffmpeg: &str, platform: Option<&str>) -> Vec<String> {
    let platform = platform.unwrap_or_else(|| system::platform());
    let args = match platform {
        "darwin" => vec!["-hide_banner", "-f", "avfoundation", "-list_devices", "true", "-i", ""],
        "win32" => vec!["-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"],
        _ => return Vec::new(),
    };
    let mut command = tokio::process::Command::new(ffmpeg);
    command.args(args).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(15), command.output()).await else { return Vec::new() };
    parse_microphones(&String::from_utf8_lossy(&output.stderr), platform)
}
pub fn terminal_app(env: &Env) -> &'static str {
    match env.get("__CFBundleIdentifier").map(String::as_str).unwrap_or("") {
        "com.apple.Terminal" => "Terminal",
        "com.googlecode.iterm2" => "iTerm2",
        "com.mitchellh.ghostty" => "Ghostty",
        "com.github.wez.wezterm" => "WezTerm",
        "net.kovidgoyal.kitty" => "kitty",
        "org.alacritty" => "Alacritty",
        "dev.warp.Warp-Stable" => "Warp",
        "com.microsoft.VSCode" => "Visual Studio Code",
        "com.todesktop.230313mzl4w4u92" => "Cursor",
        _ => match env.get("TERM_PROGRAM").map(String::as_str).unwrap_or("") {
            "Apple_Terminal" => "Terminal",
            "iTerm.app" => "iTerm2",
            "ghostty" => "Ghostty",
            "WezTerm" => "WezTerm",
            "vscode" => "Visual Studio Code",
            "WarpTerminal" => "Warp",
            _ => "your terminal app",
        },
    }
}
/// Read AVCaptureDevice's status directly. This does not request microphone access.
pub async fn microphone_allowed(platform: Option<&str>) -> Option<bool> {
    if platform.unwrap_or_else(|| system::platform()) != "darwin" {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::{c_char, c_void};
        #[link(name = "AVFoundation", kind = "framework")]
        unsafe extern "C" {}
        #[link(name = "Foundation", kind = "framework")]
        unsafe extern "C" {}
        #[link(name = "objc")]
        unsafe extern "C" {
            fn objc_getClass(name: *const c_char) -> *mut c_void;
            fn sel_registerName(name: *const c_char) -> *mut c_void;
            fn objc_msgSend();
            fn objc_autoreleasePoolPush() -> *mut c_void;
            fn objc_autoreleasePoolPop(pool: *mut c_void);
        }
        // ObjC's untyped dispatcher is called through signatures matching these two class methods.
        let status = unsafe {
            let pool = objc_autoreleasePoolPush();
            let string: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void =
                std::mem::transmute(objc_msgSend as *const ());
            let auth: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> isize = std::mem::transmute(objc_msgSend as *const ());
            let media = string(objc_getClass(c"NSString".as_ptr()), sel_registerName(c"stringWithUTF8String:".as_ptr()), c"soun".as_ptr());
            let device = objc_getClass(c"AVCaptureDevice".as_ptr());
            let status =
                if device.is_null() { 0 } else { auth(device, sel_registerName(c"authorizationStatusForMediaType:".as_ptr()), media) };
            objc_autoreleasePoolPop(pool);
            status
        };
        match status {
            3 => Some(true),
            1 | 2 => Some(false),
            _ => None,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
#[derive(Debug, Clone)]
pub struct Meter {
    bins: [u32; 101],
    levels: Vec<f64>,
    part: [i16; BLOCK],
    filled: usize,
    speech: usize,
    last_speech: isize,
    loudest: f64,
    pub peak: i32,
}
impl Default for Meter {
    fn default() -> Self {
        Self { bins: [0; 101], levels: Vec::new(), part: [0; BLOCK], filled: 0, speech: 0, last_speech: -1, loudest: 0.0, peak: 0 }
    }
}
impl Meter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, samples: &[i16]) {
        for &sample in samples {
            self.peak = self.peak.max((sample as i32).abs());
            self.part[self.filled] = sample;
            self.filled += 1;
            if self.filled == BLOCK {
                self.block();
                self.filled = 0;
            }
        }
    }
    fn block(&mut self) {
        let sum = self.part.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>();
        let rms = (sum / BLOCK as f64).sqrt();
        let db = if rms > 0.0 { (-100.0f64).max(20.0 * (rms / 32768.0).log10()) } else { -100.0 };
        self.levels.push(db);
        let bin = round(-db).min(100.0) as usize;
        self.bins[bin] = self.bins[bin].wrapping_add(1);
        if db > self.threshold() {
            self.speech += 1;
            self.last_speech = self.levels.len() as isize - 1;
        }
        self.loudest = self.loudest.max(((db + 60.0) / 50.0).clamp(0.0, 1.0));
    }
    pub fn floor(&self) -> f64 {
        let mut seen = 0u64;
        for bin in (0..=100).rev() {
            seen += self.bins[bin] as u64;
            if seen as f64 >= self.levels.len() as f64 / 6.0 {
                return -(bin as f64);
            }
        }
        -100.0
    }
    fn threshold(&self) -> f64 {
        (-50.0f64).max(self.floor() + 12.0)
    }
    pub fn take(&mut self) -> f64 {
        let level = self.loudest;
        self.loudest = 0.0;
        level
    }
    pub fn speaking(&self) -> bool {
        self.speech as f64 * BLOCK_MS >= 150.0
    }
    pub fn spoke(&self) -> bool {
        let threshold = self.threshold();
        self.levels.iter().filter(|&&l| l > threshold).take(5).count() as f64 * BLOCK_MS >= 150.0
    }
    pub fn quiet_ms(&self) -> f64 {
        (self.levels.len() as isize - 1 - self.last_speech) as f64 * BLOCK_MS
    }
}
#[derive(Default)]
struct CaptureState {
    pcm: Vec<u8>,
    started: bool,
    closed: bool,
    stopped: bool,
    ended: Option<Option<String>>,
}
#[derive(Clone)]
pub struct Capture {
    pub meter: Arc<Mutex<Meter>>,
    state: Arc<Mutex<CaptureState>>,
    changed: Arc<Notify>,
    stop_signal: Signal,
}
impl Capture {
    pub fn seconds(&self) -> f64 {
        self.state.lock().unwrap().pcm.len() as f64 / 2.0 / RATE as f64
    }
    pub fn pcm(&self) -> Vec<u8> {
        self.state.lock().unwrap().pcm.clone()
    }
    pub async fn started(&self) {
        loop {
            let changed = self.changed.notified();
            if self.state.lock().unwrap().started {
                return;
            }
            changed.await;
        }
    }
    pub async fn ended(&self) -> Option<String> {
        loop {
            let changed = self.changed.notified();
            if let Some(ended) = self.state.lock().unwrap().ended.clone() {
                return ended;
            }
            changed.await;
        }
    }
    async fn closed(&self) {
        loop {
            let changed = self.changed.notified();
            if self.state.lock().unwrap().closed {
                return;
            }
            changed.await;
        }
    }
    pub fn cancel(&self) {
        self.state.lock().unwrap().stopped = true;
        self.stop_signal.cancel();
    }
    pub async fn stop(&self) {
        self.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(1), self.closed()).await;
    }
}
/// Native process actor: capture continues while callers observe its meter or await completion.
pub fn start_capture(ffmpeg: &str, input: &[String]) -> Capture {
    let capture = Capture {
        meter: Arc::new(Mutex::new(Meter::new())),
        state: Arc::new(Mutex::new(CaptureState::default())),
        changed: Arc::new(Notify::new()),
        stop_signal: Signal::new(),
    };
    let mut command = tokio::process::Command::new(ffmpeg);
    command
        .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
        .args(input)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "-flush_packets", "1", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let mut state = capture.state.lock().unwrap();
            state.closed = true;
            state.ended = Some(Some(format!(
                "spawn {ffmpeg} {}",
                match error.kind() {
                    std::io::ErrorKind::NotFound => "ENOENT".into(),
                    std::io::ErrorKind::PermissionDenied => "EACCES".into(),
                    _ => error.to_string(),
                }
            )));
            drop(state);
            return capture;
        }
    };
    let reader = capture.clone();
    tokio::spawn(async move {
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut out_open = true;
        let mut err_open = true;
        let mut stopping = false;
        let mut odd = None;
        let mut tail = String::new();
        let mut decoder = encoding_rs::UTF_8.new_decoder_without_bom_handling();
        let mut out = [0u8; 65536];
        let mut err = [0u8; 8192];
        let status = loop {
            tokio::select! {
                read=stdout.read(&mut out),if out_open=>{match read{Ok(0)|Err(_)=>out_open=false,Ok(n)=>{let mut state=reader.state.lock().unwrap();if state.stopped{continue;}let mut data=Vec::with_capacity(n+1);if let Some(byte)=odd.take(){data.push(byte);}data.extend_from_slice(&out[..n]);let whole=data.len()-data.len()%2;odd=data.get(whole).copied();if whole==0||state.pcm.len()>=MAX_BYTES{continue;}state.pcm.extend_from_slice(&data[..whole]);reader.meter.lock().unwrap().push(&data[..whole].chunks_exact(2).map(|s|i16::from_le_bytes([s[0],s[1]])).collect::<Vec<_>>());state.started=true;drop(state);reader.changed.notify_waiters();}}},
                read=stderr.read(&mut err),if err_open=>{match read{Ok(n)=>{let mut chunk=String::with_capacity(n*3+4);let _=decoder.decode_to_string(&err[..n],&mut chunk,n==0);tail=slice(&format!("{tail}{chunk}"),-2000,None);if n==0{err_open=false;}},Err(_)=>err_open=false}},
                status=child.wait(),if !out_open&&!err_open=>break status,
                _=reader.stop_signal.cancelled(),if !stopping=>{stopping=true;#[cfg(unix)]if let Some(pid)=child.id(){unsafe{libc::kill(pid as i32,libc::SIGTERM);}}#[cfg(not(unix))]let _=child.start_kill();},
            }
        };
        let mut state = reader.state.lock().unwrap();
        state.closed = true;
        if !state.stopped {
            state.ended = Some(match status {
                Ok(s) if s.success() => None,
                Ok(s) => Some(
                    trim(&tail)
                        .lines()
                        .filter(|s| !s.is_empty())
                        .next_back()
                        .map(|s| head(s, 300))
                        .unwrap_or_else(|| format!("ffmpeg stopped ({})", s.code().map_or("null".into(), |c| c.to_string()))),
                ),
                Err(e) => Some(e.to_string()),
            });
        }
        drop(state);
        reader.changed.notify_waiters();
    });
    capture
}
