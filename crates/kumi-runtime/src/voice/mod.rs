pub mod microphone;
use crate::{
    system::{self, Env},
    video::{
        programs::{self, FfmpegOptions, OnFetch, OnProgress, ProgramOptions, VideoFailure},
        speech::{speech_model_for, transcribe, TranscribeOptions},
    },
};
use kumi_common::{
    abort::{Aborted, Signal, SignalExt},
    js::{
        number::{round, to_string},
        string::{head, trim, utf16_len},
    },
};
pub use microphone::{list_microphones, microphone_allowed, parse_microphones, terminal_app, Capture};
use microphone::{microphone_input, start_capture, RATE};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, LazyLock,
    },
    time::Duration,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceTrouble {
    Missing,
    Permission,
    Device,
    Silence,
    Quiet,
    Words,
    Failed,
}
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct VoiceError {
    pub trouble: VoiceTrouble,
    pub message: String,
}
#[derive(Debug, Clone, thiserror::Error)]
pub enum VoiceFailure {
    #[error(transparent)]
    Voice(#[from] VoiceError),
    #[error(transparent)]
    Aborted(#[from] Aborted),
    #[error("{0}")]
    Other(String),
}
impl From<std::io::Error> for VoiceFailure {
    fn from(e: std::io::Error) -> Self {
        Self::Other(e.to_string())
    }
}
impl From<VideoFailure> for VoiceFailure {
    fn from(e: VideoFailure) -> Self {
        match e {
            VideoFailure::Aborted => Self::Aborted(Aborted),
            _ => Self::Other(e.to_string()),
        }
    }
}
fn trouble(trouble: VoiceTrouble, message: impl Into<String>) -> VoiceError {
    VoiceError { trouble, message: message.into() }
}
#[derive(Debug, Clone, Default)]
pub struct Heard {
    pub pcm: Vec<u8>,
    pub seconds: f64,
    pub peak: i32,
    pub spoke: bool,
}
#[derive(Clone)]
pub struct Listening {
    capture: Capture,
}
impl Listening {
    pub fn seconds(&self) -> f64 {
        self.capture.seconds()
    }
    pub fn spoke(&self) -> bool {
        self.capture.meter.lock().unwrap().speaking()
    }
    pub fn quiet_ms(&self) -> f64 {
        self.capture.meter.lock().unwrap().quiet_ms()
    }
    pub fn level(&self) -> f64 {
        self.capture.meter.lock().unwrap().take()
    }
    pub async fn stop(&self) -> Heard {
        tokio::time::sleep(Duration::from_millis(250)).await;
        self.capture.stop().await;
        let meter = self.capture.meter.lock().unwrap();
        Heard { pcm: self.capture.pcm(), seconds: self.capture.seconds(), peak: meter.peak, spoke: meter.spoke() }
    }
    pub fn cancel(&self) {
        self.capture.cancel();
    }
    pub async fn ended(&self) -> Option<VoiceError> {
        self.capture.ended().await.map(|why| trouble(VoiceTrouble::Device, format!("The microphone stopped ({why}).")))
    }
}
#[derive(Clone, Default)]
pub struct VoiceOptions {
    pub env: Option<Env>,
    pub tools_dir: String,
    pub platform: Option<String>,
    pub signal: Option<Signal>,
    pub on_fetch: Option<OnFetch>,
    pub on_progress: Option<OnFetch>,
}
#[derive(Clone, Default)]
pub struct ListenOptions {
    pub voice: VoiceOptions,
    pub microphone: Option<String>,
    pub ffmpeg: Option<String>,
}
#[derive(Clone, Default)]
pub struct WriteDownOptions {
    pub voice: VoiceOptions,
    pub language: Option<String>,
    pub names: Vec<String>,
}
#[derive(Clone, Default)]
pub struct PrepareVoiceOptions {
    pub voice: VoiceOptions,
    pub language: Option<String>,
}
#[derive(Clone, Default)]
pub struct ReadinessOptions {
    pub env: Option<Env>,
    pub tools_dir: String,
    pub platform: Option<String>,
    pub language: Option<String>,
}
fn fetching(what: &str, progress: &Option<OnFetch>) -> Option<OnProgress> {
    progress.as_ref().map(|progress| {
        let progress = progress.clone();
        let what = what.to_string();
        Arc::new(move |fraction: f64| progress(&format!("getting {what} · {}%", to_string(round(fraction * 100.0))))) as OnProgress
    })
}
fn missing(platform: &str, ffmpeg: bool, whisper: bool) -> VoiceError {
    let both = !ffmpeg && !whisper;
    let what = if both {
        "ffmpeg, which hears the microphone, and whisper.cpp, which writes down what you say on this computer"
    } else if !ffmpeg {
        "ffmpeg, which hears the microphone"
    } else {
        "whisper.cpp, which writes down what you say on this computer"
    };
    let install = if platform == "darwin" {
        format!(
            "brew install {}",
            [(!ffmpeg).then_some("ffmpeg"), (!whisper).then_some("whisper-cpp")].into_iter().flatten().collect::<Vec<_>>().join(" ")
        )
    } else {
        [(!ffmpeg).then(programs::ffmpeg_hint), (!whisper).then(programs::whisper_hint)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; ")
    };
    trouble(VoiceTrouble::Missing, format!("Talking to Kumi needs {what}. Install {}: {install}", if both { "them" } else { "it" }))
}
fn kept_quiet(platform: &str, env: &Env, after: &str) -> String {
    match platform {
        "darwin" => {
            if after == "open" {
                format!(
                    " If macOS asked whether {} may use the microphone, allow it (System Settings › Privacy & Security › Microphone).",
                    terminal_app(env)
                )
            } else {
                format!(" Allow {} in System Settings › Privacy & Security › Microphone (macOS may just have asked), and check that the microphone isn't muted.",terminal_app(env))
            }
        }
        "win32" => format!(
            " Check that desktop apps may use the microphone (Settings › Privacy & security › Microphone){}.",
            if after == "open" { " and that no other app holds it" } else { " and that it isn't muted" }
        ),
        _ => {
            if after == "silence" {
                " Check that it isn't muted.".into()
            } else {
                String::new()
            }
        }
    }
}
static ALLOWED: AtomicBool = AtomicBool::new(false);
fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}
fn check(signal: &Option<Signal>) -> Result<(), Aborted> {
    if let Some(signal) = signal {
        signal.check()?;
    }
    Ok(())
}
async fn stopped(signal: &Option<Signal>) {
    if let Some(signal) = signal {
        signal.cancelled().await;
    } else {
        std::future::pending::<()>().await;
    }
}
async fn open(ffmpeg: &str, input: &[String], signal: &Option<Signal>) -> Result<(Capture, Option<String>), VoiceFailure> {
    check(signal)?;
    let capture = start_capture(ffmpeg, input);
    let failed = tokio::select! {biased;_=capture.started()=>None,why=capture.ended()=>Some(why.unwrap_or("it sent no sound".into())),_=stopped(signal)=>Some("stopped".into())};
    if failed.is_some() || signal.as_ref().is_some_and(Signal::is_cancelled) {
        capture.cancel();
        check(signal)?;
    }
    Ok((capture, failed))
}
pub async fn listen(options: ListenOptions) -> Result<Listening, VoiceFailure> {
    let env = options.voice.env.clone().unwrap_or_else(system::process_env);
    let platform = options.voice.platform.as_deref().unwrap_or_else(|| system::platform());
    let file = env.get("KUMI_VOICE_INPUT").filter(|s| !s.is_empty());
    if let Some(file) = file {
        if !Path::new(file).exists() {
            return Err(trouble(VoiceTrouble::Device, format!("KUMI_VOICE_INPUT names {file}, which isn't there.")).into());
        }
    }
    let fetches = programs::whisper_asset(platform, arch()).is_some();
    let (ffmpeg, whisper) = tokio::try_join!(
        async {
            if let Some(ffmpeg) = options.ffmpeg {
                Ok(Some(ffmpeg))
            } else {
                programs::find_ffmpeg(FfmpegOptions {
                    env: Some(env.clone()),
                    tools_dir: Some(options.voice.tools_dir.clone()),
                    purpose: Some("which it hears the microphone through".into()),
                    signal: options.voice.signal.clone(),
                    on_fetch: options.voice.on_fetch.clone(),
                    on_progress: fetching("ffmpeg", &options.voice.on_progress),
                    ..Default::default()
                })
                .await
            }
        },
        async {
            if fetches {
                Ok(true)
            } else {
                programs::find_whisper(&ProgramOptions {
                    env: Some(env.clone()),
                    tools_dir: options.voice.tools_dir.clone(),
                    installed_only: true,
                    ..Default::default()
                })
                .await
                .map(|w| w.is_some())
            }
        }
    )?;
    if !fetches && (ffmpeg.is_none() || !whisper) {
        return Err(missing(platform, ffmpeg.is_some(), whisper).into());
    }
    let Some(ffmpeg) = ffmpeg else {
        return Err(trouble(VoiceTrouble::Missing,format!("Kumi hears the microphone through ffmpeg, and couldn't fetch it just now. Check the connection and try again, or install it: {}",programs::ffmpeg_hint())).into());
    };
    if file.is_none() && !ALLOWED.load(Ordering::Relaxed) {
        let answer = microphone_allowed(Some(platform)).await;
        if answer == Some(false) {
            return Err(trouble(VoiceTrouble::Permission,format!("macOS isn't letting {} use the microphone. Allow it in System Settings › Privacy & Security › Microphone, then try again.",terminal_app(&env))).into());
        }
        ALLOWED.store(answer == Some(true), Ordering::Relaxed);
    }
    let mut device = options.microphone;
    if file.is_none() && platform == "win32" && device.as_ref().is_none_or(|d| d.is_empty()) {
        device = list_microphones(&ffmpeg, Some(platform)).await.into_iter().next();
        if device.is_none() {
            return Err(trouble(VoiceTrouble::Device,"Kumi found no microphone on this computer. Plug one in, or check that Windows sees it (Settings › System › Sound › Input).").into());
        }
    }
    let input =
        file.map(|file| vec!["-re".into(), "-i".into(), file.clone()]).unwrap_or_else(|| microphone_input(platform, device.as_deref()));
    let (mut capture, mut failed) = open(&ffmpeg, &input, &options.voice.signal).await?;
    if failed.as_ref().is_some_and(|f| f.to_ascii_lowercase().contains("pulse")) && file.is_none() && platform == "linux" {
        (capture, failed) = open(
            &ffmpeg,
            &["-f".into(), "alsa".into(), "-i".into(), device.filter(|d| !d.is_empty()).unwrap_or("default".into())],
            &options.voice.signal,
        )
        .await?;
    }
    if let Some(failed) = failed {
        return Err(trouble(
            VoiceTrouble::Device,
            format!("Kumi couldn't open the microphone ({failed}).{}", kept_quiet(platform, &env, "open")),
        )
        .into());
    }
    Ok(Listening { capture })
}
pub fn wav_file(pcm: &[u8]) -> Vec<u8> {
    let mut file = vec![0; 44];
    file[0..4].copy_from_slice(b"RIFF");
    file[4..8].copy_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    file[8..12].copy_from_slice(b"WAVE");
    file[12..16].copy_from_slice(b"fmt ");
    file[16..20].copy_from_slice(&16u32.to_le_bytes());
    file[20..22].copy_from_slice(&1u16.to_le_bytes());
    file[22..24].copy_from_slice(&1u16.to_le_bytes());
    file[24..28].copy_from_slice(&(RATE as u32).to_le_bytes());
    file[28..32].copy_from_slice(&((RATE * 2) as u32).to_le_bytes());
    file[32..34].copy_from_slice(&2u16.to_le_bytes());
    file[34..36].copy_from_slice(&16u16.to_le_bytes());
    file[36..40].copy_from_slice(b"data");
    file[40..44].copy_from_slice(&(pcm.len() as u32).to_le_bytes());
    file.extend_from_slice(pcm);
    file
}
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s\u{feff}]+").unwrap());
pub fn voice_prompt(names: &[String]) -> String {
    let mut own = Vec::new();
    for name in names {
        let name = trim(&SPACES.replace_all(name, " ")).to_string();
        if !name.is_empty() && utf16_len(&name) <= 40 && !own.contains(&name) {
            own.push(name);
            if own.len() == 12 {
                break;
            }
        }
    }
    format!("A producer talks to Kumi about the bass, the kick, the hats and the pad in their Ableton Live Set{}, with Operator, Wavetable, Drift, Simpler, Drum Rack, Saturator, Roar, EQ Eight, Glue Compressor and Auto Filter.",if own.is_empty(){String::new()}else{format!(" ({})",own.join(", "))})
}
pub fn clean_words(text: &str) -> String {
    static MARKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]*\]|\*[^*]*\*").unwrap());
    let text = MARKS.replace_all(text, " ");
    let words = trim(&SPACES.replace_all(&text, " ")).to_string();
    let bare = words.to_lowercase();
    let bare = bare
        .trim_end_matches(|c: char| ".!?,。！？、".contains(c) || c.is_whitespace() || c == '\u{feff}')
        .trim_start_matches(|c: char| ".!?,".contains(c) || c.is_whitespace() || c == '\u{feff}');
    if bare.is_empty()
        || [
            "you",
            "thank you",
            "thanks",
            "thank you for watching",
            "thanks for watching",
            "bye",
            "the end",
            "subscribe",
            "please subscribe",
            "ご視聴ありがとうございました",
        ]
        .contains(&bare)
    {
        String::new()
    } else {
        words
    }
}
pub fn audio_context_for(seconds: f64) -> f64 {
    (((seconds + 2.0) * 50.0 / 64.0).ceil() * 64.0).min(1500.0)
}
fn program_options(options: &VoiceOptions, env: &Env, what: &str, purpose: &str) -> ProgramOptions {
    ProgramOptions {
        env: Some(env.clone()),
        tools_dir: options.tools_dir.clone(),
        purpose: Some(purpose.into()),
        signal: options.signal.clone(),
        on_fetch: options.on_fetch.clone(),
        on_progress: fetching(what, &options.on_progress),
        ..Default::default()
    }
}
pub async fn write_down(heard: Heard, options: WriteDownOptions) -> Result<String, VoiceFailure> {
    let env = options.voice.env.clone().unwrap_or_else(system::process_env);
    let platform = options.voice.platform.as_deref().unwrap_or_else(|| system::platform());
    if heard.pcm.is_empty() || heard.peak == 0 {
        return Err(trouble(
            VoiceTrouble::Silence,
            format!("Kumi got only silence from the microphone.{}", kept_quiet(platform, &env, "silence")),
        )
        .into());
    }
    if !heard.spoke {
        return Err(trouble(
            VoiceTrouble::Quiet,
            "Kumi didn't hear you: the microphone picked up only quiet. Speak a little closer, or check its input level.",
        )
        .into());
    }
    let whisper =
        programs::find_whisper(&program_options(&options.voice, &env, "whisper.cpp", "which writes down what you say, on this computer"))
            .await
            .map_err(|error| {
                if check(&options.voice.signal).is_err() {
                    VoiceFailure::Aborted(Aborted)
                } else {
                    trouble(
                        VoiceTrouble::Missing,
                        format!(
                            "Kumi writes down what you say with whisper.cpp, and couldn't fetch it just now ({}).",
                            head(&error.to_string(), 160)
                        ),
                    )
                    .into()
                }
            })?;
    let Some(whisper) = whisper else { return Err(missing(platform, true, false).into()) };
    let language = options.language.as_deref().unwrap_or("en");
    let (model, vad) = tokio::join!(
        async {
            programs::whisper_model(
                speech_model_for(Some(language)),
                &program_options(&options.voice, &env, "the speech model", "to write down what you say"),
            )
            .await
            .map_err(|error| {
                if check(&options.voice.signal).is_err() {
                    VoiceFailure::Aborted(Aborted)
                } else {
                    trouble(
                        VoiceTrouble::Missing,
                        format!(
                            "Kumi needs a speech model to write down what you say, and couldn't fetch it ({}).",
                            head(&error.to_string(), 200)
                        ),
                    )
                    .into()
                }
            })
        },
        async {
            programs::vad_model(&ProgramOptions {
                env: Some(env.clone()),
                tools_dir: options.voice.tools_dir.clone(),
                signal: options.voice.signal.clone(),
                ..Default::default()
            })
            .await
            .ok()
        }
    );
    let model = model?;
    let folder = std::env::temp_dir().join(format!("kumi-voice-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&folder).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).await?;
    }
    let result = async {
        let wav = folder.join("voice.wav");
        let mut create = tokio::fs::OpenOptions::new();
        create.write(true).create_new(true);
        #[cfg(unix)]
        create.mode(0o600);
        use tokio::io::AsyncWriteExt;
        let mut file = create.open(&wav).await?;
        file.write_all(&wav_file(&heard.pcm)).await?;
        file.flush().await?;
        drop(file);
        let write = |vad: Option<String>| {
            transcribe(
                &whisper,
                &model,
                wav.to_str().unwrap(),
                TranscribeOptions {
                    language: Some(language.into()),
                    audio_context: Some(audio_context_for(heard.seconds)),
                    timeout_ms: Some(120_000),
                    prompt: if !language.is_empty() && speech_model_for(Some(language)).contains(".en") {
                        Some(voice_prompt(&options.names))
                    } else {
                        None
                    },
                    vad,
                    signal: options.voice.signal.clone(),
                    ..Default::default()
                },
            )
        };
        let mut cues = write(vad.clone()).await;
        if cues.is_err() {
            check(&options.voice.signal)?;
            if vad.is_some() {
                cues = write(None).await;
            }
        }
        let cues = cues.map_err(|error| {
            if check(&options.voice.signal).is_err() {
                VoiceFailure::Aborted(Aborted)
            } else {
                trouble(VoiceTrouble::Failed, format!("Kumi couldn't write down what you said ({}).", head(&error.to_string(), 200))).into()
            }
        })?;
        let words = clean_words(&cues.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" "));
        if words.is_empty() {
            return Err(
                trouble(VoiceTrouble::Words, "Kumi couldn't make out any words. Try again, a little closer to the microphone.").into()
            );
        }
        Ok(words)
    }
    .await;
    let _ = tokio::fs::remove_dir_all(folder).await;
    result
}
pub async fn prepare_voice(options: PrepareVoiceOptions) -> Result<(), VoiceFailure> {
    let env = options.voice.env.clone().unwrap_or_else(system::process_env);
    programs::find_whisper(&program_options(&options.voice, &env, "whisper.cpp", "which writes down what you say, on this computer"))
        .await?;
    programs::whisper_model(
        speech_model_for(Some(options.language.as_deref().unwrap_or("en"))),
        &program_options(&options.voice, &env, "the speech model", "to write down what you say"),
    )
    .await?;
    programs::vad_model(&ProgramOptions {
        env: Some(env),
        tools_dir: options.voice.tools_dir,
        signal: options.voice.signal,
        on_fetch: options.voice.on_fetch,
        ..Default::default()
    })
    .await?;
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelReadiness {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceReadiness {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ffmpeg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whisper: Option<String>,
    pub model: ModelReadiness,
    pub fetches: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed: Option<bool>,
}
pub async fn voice_readiness(options: ReadinessOptions) -> Result<VoiceReadiness, VoiceFailure> {
    let env = options.env.unwrap_or_else(system::process_env);
    let platform = options.platform.as_deref().unwrap_or_else(|| system::platform());
    let (ffmpeg, whisper, allowed) = tokio::join!(
        programs::find_ffmpeg(FfmpegOptions {
            env: Some(env.clone()),
            tools_dir: Some(options.tools_dir.clone()),
            installed_only: true,
            ..Default::default()
        }),
        async {
            programs::find_whisper(&ProgramOptions {
                env: Some(env.clone()),
                tools_dir: options.tools_dir.clone(),
                installed_only: true,
                ..Default::default()
            })
            .await
        },
        microphone_allowed(Some(platform))
    );
    let name = speech_model_for(Some(options.language.as_deref().unwrap_or("en")));
    let path = env
        .get("KUMI_WHISPER_MODEL")
        .cloned()
        .unwrap_or_else(|| Path::new(&options.tools_dir).join("whisper-models").join(name).to_string_lossy().into());
    let present = std::fs::metadata(&path).is_ok_and(|m| m.len() > 0);
    Ok(VoiceReadiness {
        ffmpeg: ffmpeg?,
        whisper: whisper?,
        model: ModelReadiness { name: name.into(), path: present.then_some(path) },
        fetches: programs::whisper_asset(platform, arch()).is_some(),
        allowed,
    })
}
