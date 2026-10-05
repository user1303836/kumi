//! whisper.cpp speech transcription on this computer.
use super::{captions::Cue, programs::VideoFailure};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{
        number::{parse, round, to_string},
        string::{head, slice, trim},
    },
};
use regex::Regex;
use serde_json::Value;
use std::{path::Path, process::Stdio, rc::Rc, sync::LazyLock, time::Duration};
use tokio::io::AsyncReadExt;
pub fn speech_model_for(language: Option<&str>) -> &'static str {
    let language = language.unwrap_or("");
    let lower = language.to_ascii_lowercase();
    if language.is_empty()
        || lower.strip_prefix("en").is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_'))
    {
        "ggml-small.en-q5_1.bin"
    } else {
        "ggml-small-q5_1.bin"
    }
}
#[derive(Clone, Default)]
pub struct TranscribeOptions {
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub signal: Option<Signal>,
    pub on_progress: Option<Rc<dyn Fn(f64)>>,
    pub timeout_ms: Option<u64>,
    pub audio_context: Option<f64>,
    pub vad: Option<String>,
}
pub fn cues_from_whisper(text: &str) -> Vec<Cue> {
    let Ok(result) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    static MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\[(].*[\])]$").unwrap());
    let number = |v: Option<&Value>| match v {
        Some(Value::Null) => 0.0,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::Bool(b)) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Some(v) => parse(&crate::web::net::js_string(v)).unwrap_or(f64::NAN),
        None => f64::NAN,
    };
    result["transcription"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|segment| {
            let text = segment.get("text").filter(|v| !v.is_null()).map(crate::web::net::js_string).unwrap_or_default();
            let text = trim(&SPACES.replace_all(&text, " ")).to_string();
            let start = number(segment["offsets"].get("from")) / 1000.0;
            let end = number(segment["offsets"].get("to")) / 1000.0;
            (!text.is_empty() && !MARK.is_match(&text) && start.is_finite() && end.is_finite()).then_some(Cue { start, end, text })
        })
        .collect()
}
pub fn speech_prompt(title: &str) -> String {
    format!("{}. Ableton Live tutorial: Operator, Wavetable, Drift, Simpler, Sampler, Drum Rack, Instrument Rack, Saturator, Dynamic Tube, EQ Eight, EQ Three, Glue Compressor, OTT, Roar, Auto Filter, Utility, reverb, delay, LFO, envelope, oscillator, detune, sidechain, dry/wet, Serum, Vital.",head(title,200))
}
async fn cancelled(signal: &Option<Signal>) {
    if let Some(signal) = signal {
        signal.cancelled().await;
    } else {
        std::future::pending::<()>().await;
    }
}
pub async fn transcribe(whisper: &str, model: &str, wav: &str, options: TranscribeOptions) -> Result<Vec<Cue>, VideoFailure> {
    let out = Path::new(wav)
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(".transcript-{}", uuid::Uuid::new_v4()))
        .to_string_lossy()
        .into_owned();
    let language = options.language.as_deref().unwrap_or("").split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    let language = if model.contains(".en") {
        "en"
    } else if (2..=3).contains(&language.len()) && language.bytes().all(|b| b.is_ascii_lowercase()) {
        &language
    } else {
        "auto"
    };
    // One guess at each step (`-bs 1`) rather than whisper.cpp's beam of five: a tutorial's words come
    // out the same, in about two-thirds of the time. A stretch it's unsure of is still tried five ways.
    // Its threads stay its own default (up to 4), leaving Live the rest.
    let mut args: Vec<String> =
        ["-m", model, "-f", wav, "-oj", "-of", &out, "-pp", "-sns", "-bs", "1", "-l", language].into_iter().map(str::to_string).collect();
    if let Some(prompt) = options.prompt.as_deref().filter(|s| !s.is_empty()) {
        args.extend(["--prompt".into(), head(prompt, 600)]);
    }
    if let Some(context) = options.audio_context.filter(|c| *c != 0.0 && *c < 1500.0) {
        args.extend(["-ac".into(), to_string(round(context).max(64.0))]);
    }
    if let Some(vad) = options.vad.filter(|s| !s.is_empty()) {
        args.extend(["--vad".into(), "-vm".into(), vad]);
    }
    if let Some(signal) = &options.signal {
        signal.check()?;
    }
    let mut command = tokio::process::Command::new(whisper);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|e| {
        VideoFailure::other(format!(
            "spawn {whisper} {}",
            match e.kind() {
                std::io::ErrorKind::NotFound => "ENOENT".into(),
                std::io::ErrorKind::PermissionDenied => "EACCES".into(),
                _ => e.to_string(),
            }
        ))
    })?;
    let mut stderr = child.stderr.take().unwrap();
    let mut tail = String::new();
    let timer = tokio::time::sleep(Duration::from_millis(options.timeout_ms.unwrap_or(900_000)));
    tokio::pin!(timer);
    let mut timed_out = false;
    let mut open = true;
    let mut buffer = [0; 8192];
    let mut decoder = encoding_rs::UTF_8.new_decoder_without_bom_handling();
    static PROGRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"progress\s*=\s*([0-9]+)%").unwrap());
    let status = loop {
        tokio::select! {
            read=stderr.read(&mut buffer),if open=>{let count=read?;let mut chunk=String::with_capacity(count*3+4);let _=decoder.decode_to_string(&buffer[..count],&mut chunk,count==0);if count==0{open=false;}tail=slice(&format!("{tail}{chunk}"),-4000,None);if let Some(progress)=&options.on_progress{for c in PROGRESS.captures_iter(&chunk){progress(c[1].parse().unwrap_or(0.0));}}},
            status=child.wait(),if !open=>break status?,
            _=&mut timer,if !timed_out=>{timed_out=true;#[cfg(unix)]if let Some(pid)=child.id(){unsafe{libc::kill(pid as i32,libc::SIGTERM);}}#[cfg(not(unix))]let _=child.start_kill();},
            _=cancelled(&options.signal)=>{let _=child.start_kill();let _=child.wait().await;let _=tokio::fs::remove_file(format!("{out}.json")).await;return Err(VideoFailure::Aborted);}
        }
    };
    let result = if !status.success() {
        Err(VideoFailure::other(trim(&tail).split('\n').filter(|s| !s.is_empty()).next_back().map_or_else(
            || format!("whisper.cpp stopped ({})", status.code().map_or_else(|| "null".into(), |c| c.to_string())),
            |line| head(line, 300),
        )))
    } else {
        tokio::fs::read_to_string(format!("{out}.json")).await.map(|text| cues_from_whisper(&text)).map_err(VideoFailure::from)
    };
    let _ = tokio::fs::remove_file(format!("{out}.json")).await;
    result
}
