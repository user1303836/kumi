//! The listening model: an audio-capable model hears a change beside the meters. It judges style and obvious problems
//! (harsh, muddy, distorted, pumping); the meters decide anything at the dB level, since no model reliably hears a
//! 2 dB peak. Kumi asks it as an A/B ("which is closer to the aim?"), the two takes joined with a pause, then again with
//! the order swapped, because models favor a position: only an answer that holds both ways counts.
//!
//! A model on this computer named in `KUMI_LISTENER` (`<base url>#<model>`, any OpenAI-compatible chat endpoint that
//! takes audio: a llama.cpp server with Qwen3-Omni, say), else Gemini with a Gemini API key, else OpenAI's audio
//! models with an OpenAI API key, each picked from the provider's own model list. `KUMI_LISTENER=off` leaves the meters
//! alone.

use crate::audio::decode::open_audio;
use async_trait::async_trait;
use base64::Engine;
use kumi_common::abort::{Signal, SignalExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

/// A take to compare: a file and the stretch of it to play.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    pub file: std::path::PathBuf,
    pub start: f64,
    pub seconds: f64,
    /// dB to bring it to the other take's loudness.
    pub gain: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    Before,
    After,
    Same,
}

/// What the listener heard: which take is closer to the aim (when both orders agree), and its words.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opinion {
    pub closer: Option<Choice>,
    /// Problems it heard in the change that the take before didn't have (both orders agreeing).
    pub new_problems: Vec<String>,
    pub said: String,
}

impl Opinion {
    /// The opinion in a line for the round log.
    pub fn line(&self, model: &str) -> String {
        let closer = match self.closer {
            Some(Choice::After) => "prefers the change".to_string(),
            Some(Choice::Before) => "prefers it before the change".to_string(),
            Some(Choice::Same) => "hears no difference".to_string(),
            None => "isn't sure (its answer changed with the order)".to_string(),
        };
        let problems = if self.new_problems.is_empty() { String::new() } else { format!("; hears it {}", self.new_problems.join(", ")) };
        format!("{model} {closer}{problems}")
    }
}

#[async_trait(?Send)]
pub trait Listener {
    /// The model, as the log names it.
    fn name(&self) -> String;
    /// Whether it hears stereo and the top octave: one that hears a mono downmix at a low rate isn't asked about width,
    /// phase or air.
    fn hears_width(&self) -> bool {
        true
    }
    /// Turned off since it was found: the listening slot now says off.
    fn off(&self) -> bool {
        false
    }
    /// Asks once: `first` then `second`, joined with a pause, against the aim. Answers "first", "second" or "same",
    /// and the problems it hears in each.
    async fn ask(&self, wav: &[u8], aim: &str, signal: Signal) -> Result<Answer, String>;
}

/// One answer: which take is closer, and the problems heard in each.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub closer: String,
    #[serde(default)]
    pub first: Vec<String>,
    #[serde(default)]
    pub second: Vec<String>,
}

/// The A/B both ways: before then after, after then before. An answer counts only when it holds in both orders.
pub async fn compare(listener: &dyn Listener, before: &Take, after: &Take, aim: &str, signal: Signal) -> Result<Opinion, String> {
    let (b, a) = (read_take(before, signal.clone()).await?, read_take(after, signal.clone()).await?);
    let rate = b.0;
    let forward = joined(&b, &a, rate);
    let backward = joined(&a, &b, rate);
    let one = listener.ask(&forward, aim, signal.clone()).await?;
    signal.check().map_err(|error| error.to_string())?;
    let two = listener.ask(&backward, aim, signal).await?;
    let pick = |answer: &Answer, first: Choice, second: Choice| match answer.closer.to_lowercase().trim() {
        "first" | "a" => Some(first),
        "second" | "b" => Some(second),
        "same" | "neither" | "equal" => Some(Choice::Same),
        _ => None,
    };
    let (x, y) = (pick(&one, Choice::Before, Choice::After), pick(&two, Choice::After, Choice::Before));
    let closer = if x == y { x } else { None };
    // A problem heard in the change both ways, and not in the take before.
    let normal = |list: &[String]| {
        list.iter().map(|problem| problem.trim().to_lowercase()).filter(|problem| !problem.is_empty()).collect::<Vec<_>>()
    };
    let (after_one, after_two, before_one, before_two) = (normal(&one.second), normal(&two.first), normal(&one.first), normal(&two.second));
    let new_problems = after_one
        .iter()
        .filter(|problem| after_two.contains(problem) && !before_one.contains(problem) && !before_two.contains(problem))
        .cloned()
        .collect();
    Ok(Opinion { closer, new_problems, said: format!("{} / {}", one.closer, two.closer) })
}

/// A take's samples (mono, level-matched, at most 10 s) and its rate.
async fn read_take(take: &Take, signal: Signal) -> Result<(f64, Vec<f32>), String> {
    let mut source = open_audio(&take.file, Some(signal)).await.map_err(|error| error.0)?;
    let rate = source.sample_rate;
    source.seek(take.start * rate);
    let frames = (take.seconds.min(10.) * rate) as usize;
    let gain = 10f64.powf(take.gain / 20.) as f32;
    let mut out = Vec::with_capacity(frames);
    while out.len() < frames {
        let Some(block) = source.read(65536.min(frames - out.len())).await.map_err(|error| error.0)? else { break };
        let right = block.get(1).unwrap_or(&block[0]);
        out.extend(block[0].iter().zip(right).map(|(l, r)| (l + r) / 2. * gain));
    }
    let _ = source.close().await;
    Ok((rate, out))
}

/// Two takes as one 16-bit mono WAV, 0.8 s of silence between them.
fn joined(first: &(f64, Vec<f32>), second: &(f64, Vec<f32>), rate: f64) -> Vec<u8> {
    let pause = vec![0f32; (0.8 * rate) as usize];
    let samples: Vec<f32> = first.1.iter().chain(&pause).chain(&second.1).copied().collect();
    let data: Vec<u8> = samples.iter().flat_map(|sample| ((sample.clamp(-1., 1.) * 32767.) as i16).to_le_bytes()).collect();
    let rate = rate as u32;
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend(b"RIFF");
    wav.extend((36 + data.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 2).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((data.len() as u32).to_le_bytes());
    wav.extend(data);
    wav
}

/// The question: coarse and about the aim, with the answer as JSON.
pub fn question(aim: &str) -> String {
    format!(
        "You hear two takes of the same music: the first, a short pause, then the second. The aim: {aim}. Which take is closer to the aim? Also list obvious problems you hear in each (only from: harsh, muddy, boomy, thin, distorted, pumping, dull, harshly sibilant, clipped). Answer with JSON only: {{\"closer\": \"first\" | \"second\" | \"same\", \"first\": [problems], \"second\": [problems]}}."
    )
}

/// An OpenAI-compatible chat endpoint that takes audio input.
pub struct ChatListener {
    pub base: String,
    pub key: Option<String>,
    pub model: String,
    pub client: reqwest::Client,
}

#[async_trait(?Send)]
impl Listener for ChatListener {
    fn name(&self) -> String {
        self.model.clone()
    }
    async fn ask(&self, wav: &[u8], aim: &str, signal: Signal) -> Result<Answer, String> {
        let body = json!({
            "model": self.model,
            "modalities": ["text"],
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": question(aim)},
                {"type": "input_audio", "input_audio": {"data": base64::engine::general_purpose::STANDARD.encode(wav), "format": "wav"}}
            ]}]
        });
        let mut request = self.client.post(format!("{}/chat/completions", self.base.trim_end_matches('/'))).json(&body);
        if let Some(key) = &self.key {
            request = request.bearer_auth(key);
        }
        // The whole exchange, sending and reading back, gives way to Esc.
        let sent = tokio::select! {
            sent = request.send() => sent.map_err(|error| format!("the listening model didn't answer: {error}"))?,
            _ = signal.cancelled() => return Err("stopped".into()),
        };
        let status = sent.status();
        let reply: Value = tokio::select! {
            reply = sent.json() => reply.map_err(|error| format!("the listening model's answer wasn't JSON: {error}"))?,
            _ = signal.cancelled() => return Err("stopped".into()),
        };
        if !status.is_success() {
            return Err(format!("the listening model refused ({status})"));
        }
        let text = reply["choices"][0]["message"]["content"].as_str().unwrap_or("");
        parse_answer(text)
            .ok_or_else(|| format!("the listening model's answer wasn't the JSON asked for: {}", kumi_common::js::string::head(text, 120)))
    }
}

/// The JSON object in a model's answer (it may wrap it in prose or a code fence).
pub fn parse_answer(text: &str) -> Option<Answer> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

/// `KUMI_LISTENER=off`: no listening model, the meters alone (no audio leaves the computer).
pub fn listening_off(env: &std::collections::HashMap<String, String>) -> bool {
    env.get("KUMI_LISTENER").is_some_and(|value| matches!(value.trim().to_lowercase().as_str(), "off" | "none" | "0" | "false"))
}
/// A listener named in `KUMI_LISTENER` (`<base url>#<model>`, its key in `KUMI_LISTENER_KEY` when it needs one).
pub fn listener_from_env(env: &std::collections::HashMap<String, String>) -> Option<ChatListener> {
    let named = env.get("KUMI_LISTENER").filter(|value| !value.trim().is_empty())?;
    let (base, model) = named.split_once('#')?;
    Some(ChatListener {
        base: base.trim().into(),
        key: env.get("KUMI_LISTENER_KEY").cloned(),
        model: model.trim().into(),
        client: client(),
    })
}

/// A client that gives up on a model that stops answering (a minute and a half: it hears 20 s of audio).
fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(90)).build().unwrap_or_default()
}

/// OpenAI's newest audio-capable chat model for a key, from its own model list (ids that take audio: "…audio…", not
/// the realtime, speech or transcription ones). None when the list has none; an error when it couldn't be read (the
/// network, the service, Esc), which another try may get past.
pub async fn openai_listener(key: &str, signal: Signal) -> Result<Option<ChatListener>, String> {
    let client = client();
    let listed = tokio::select! {
        listed = client.get("https://api.openai.com/v1/models").bearer_auth(key).timeout(std::time::Duration::from_secs(20)).send() => listed.map_err(|error| error.to_string())?,
        _ = signal.cancelled() => return Err("stopped".into()),
    };
    if !listed.status().is_success() {
        return Err(format!("OpenAI's model list answered {}", listed.status()));
    }
    let body: Value = tokio::select! {
        body = listed.json() => body.map_err(|error| error.to_string())?,
        _ = signal.cancelled() => return Err("stopped".into()),
    };
    let model = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| Some((row["id"].as_str()?.to_string(), row["created"].as_f64().unwrap_or(0.))))
        .filter(|(id, _)| id.contains("audio") && !["realtime", "tts", "transcribe", "mini-tts"].iter().any(|word| id.contains(word)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(model, _)| model);
    Ok(model.map(|model| ChatListener { base: "https://api.openai.com/v1".into(), key: Some(key.into()), model, client }))
}

/// Whether a path is a file a listener can be given.
pub fn readable(path: &Path) -> bool {
    path.is_file()
}

/// Gemini, with a Gemini API key: the audio goes in a generateContent request, and the answer comes back as JSON.
/// Gemini hears a mono downmix at a low rate, so it isn't asked about width, phase or air.
pub struct GeminiListener {
    pub base: String,
    pub key: String,
    /// "models/…", as Gemini's list names it.
    pub model: String,
    pub client: reqwest::Client,
}

#[async_trait(?Send)]
impl Listener for GeminiListener {
    fn name(&self) -> String {
        self.model.trim_start_matches("models/").to_string()
    }
    fn hears_width(&self) -> bool {
        false
    }
    async fn ask(&self, wav: &[u8], aim: &str, signal: Signal) -> Result<Answer, String> {
        let body = json!({
            "contents": [{"parts": [
                {"text": question(aim)},
                {"inline_data": {"mime_type": "audio/wav", "data": base64::engine::general_purpose::STANDARD.encode(wav)}}
            ]}],
            "generationConfig": {"responseMimeType": "application/json"}
        });
        let address = format!("{}/v1beta/{}:generateContent", self.base.trim_end_matches('/'), self.model);
        let sent = tokio::select! {
            sent = self.client.post(address).header("x-goog-api-key", &self.key).json(&body).send() => sent.map_err(|error| format!("the listening model didn't answer: {error}"))?,
            _ = signal.cancelled() => return Err("stopped".into()),
        };
        let status = sent.status();
        let reply: Value = tokio::select! {
            reply = sent.json() => reply.map_err(|error| format!("the listening model's answer wasn't JSON: {error}"))?,
            _ = signal.cancelled() => return Err("stopped".into()),
        };
        if !status.is_success() {
            return Err(format!("the listening model refused ({status})"));
        }
        let text: String =
            reply["candidates"][0]["content"]["parts"].as_array().into_iter().flatten().filter_map(|part| part["text"].as_str()).collect();
        parse_answer(&text)
            .ok_or_else(|| format!("the listening model's answer wasn't the JSON asked for: {}", kumi_common::js::string::head(&text, 120)))
    }
}

/// Gemini's best listener for a key, from its own model list: the models that generate content from what they're
/// given (not the embedding, speech, image or live ones), the newest version first, a released one before a preview,
/// and of one version the light ("flash") one, which hears as well for a fraction of the cost. None when the list has
/// none; an error when it couldn't be read.
pub async fn gemini_listener(base: &str, key: &str, signal: Signal) -> Result<Option<GeminiListener>, String> {
    let client = client();
    let address = format!("{}/v1beta/models?pageSize=1000", base.trim_end_matches('/'));
    let listed = tokio::select! {
        listed = client.get(address).header("x-goog-api-key", key).timeout(std::time::Duration::from_secs(20)).send() => listed.map_err(|error| error.to_string())?,
        _ = signal.cancelled() => return Err("stopped".into()),
    };
    if !listed.status().is_success() {
        return Err(format!("Gemini's model list answered {}", listed.status()));
    }
    let body: Value = tokio::select! {
        body = listed.json() => body.map_err(|error| error.to_string())?,
        _ = signal.cancelled() => return Err("stopped".into()),
    };
    let model = body["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["supportedGenerationMethods"].as_array().is_some_and(|ways| ways.iter().any(|way| way == "generateContent")))
        .filter_map(|row| row["name"].as_str())
        // Gemini's own models only (Gemma's are listed too, and don't take audio), and not those for another job.
        .filter(|name| name.starts_with("models/gemini-"))
        .filter(|name| {
            !["embedding", "tts", "image", "live", "aqa", "native-audio", "transcribe", "robotics", "computer-use", "omni", "nano-banana"]
                .iter()
                .any(|word| name.contains(word))
        })
        .max_by_key(|name| gemini_rank(name))
        .map(str::to_owned);
    Ok(model.map(|model| GeminiListener { base: base.trim_end_matches('/').into(), key: key.into(), model, client }))
}

/// How a Gemini model ranks as a listener, from its name: its version, a released one over a preview or experiment,
/// then the light one of a version.
fn gemini_rank(name: &str) -> (Vec<u32>, bool, bool) {
    let version: Vec<u32> = name
        .trim_start_matches("models/")
        .split('-')
        .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|part| part.split('.').filter_map(|piece| piece.parse().ok()).collect())
        .unwrap_or_default();
    (version, !(name.contains("preview") || name.contains("exp")), name.contains("flash") && !name.contains("lite"))
}

/// Gemini's API key: saved under "gemini", else GEMINI_API_KEY or GOOGLE_API_KEY.
pub async fn gemini_key(
    store: &dyn crate::auth::store::CredentialStore,
    env: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if let Ok(Some(crate::auth::store::Credential::ApiKey { key })) = store.get("gemini").await {
        return Some(key);
    }
    ["GEMINI_API_KEY", "GOOGLE_API_KEY"].iter().find_map(|name| env.get(*name).filter(|key| !key.trim().is_empty()).cloned())
}

/// Gemini's public address.
pub const GEMINI: &str = "https://generativelanguage.googleapis.com";
