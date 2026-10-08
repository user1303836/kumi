//! Model slots: which model does each of Kumi's listening jobs. One slot per job, each with a permissive default:
//! stems with Live's own splitter, transcription with Live's conversions, listening with the lookup (a model named in
//! `KUMI_LISTENER`, else Gemini with a Gemini key, else OpenAI with an OpenAI key), and embeddings with none until one
//! is fetched. A swap is asked for in plain words, or with a model file or a Hugging Face link. A new listening model
//! is tried on a known clip first and switched to only when it hears it right; every swap is said, and `/slots back`
//! takes it back. Kumi runs no model files itself yet, so a file or a link is said to be out of reach, not switched to.

use crate::{
    auth::store::CredentialStore,
    listening::listener::{
        gemini_key, gemini_listener, listener_from_env, listening_off, openai_listener, Answer, ChatListener, Listener, GEMINI,
    },
    providers::{api_key_for, ProviderId},
};
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    f64::consts::PI,
    path::{Path, PathBuf},
    rc::Rc,
};

/// A listening job with a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Job {
    Stems,
    Transcription,
    Listening,
    Embeddings,
}

impl Job {
    pub const ALL: [Job; 4] = [Job::Stems, Job::Transcription, Job::Listening, Job::Embeddings];

    pub fn name(self) -> &'static str {
        match self {
            Job::Stems => "stems",
            Job::Transcription => "transcription",
            Job::Listening => "listening",
            Job::Embeddings => "embeddings",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Job::Stems => "a mix split into its parts",
            Job::Transcription => "audio turned into MIDI notes",
            Job::Listening => "a change heard beside the meters",
            Job::Embeddings => "how alike two sounds are",
        }
    }

    /// The words a producer may call it by.
    fn words(self) -> &'static [&'static str] {
        match self {
            Job::Stems => &["stems", "stem", "separation", "separate", "separating", "splitter", "split", "splitting"],
            Job::Transcription => &["transcription", "transcribe", "transcribing", "midi", "notes", "conversion", "conversions"],
            Job::Listening => &["listening", "listen", "listener", "ears", "hearing"],
            Job::Embeddings => &["embeddings", "embedding", "similarity", "likeness"],
        }
    }
}

/// What fills a slot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "use")]
pub enum Choice {
    /// The job's default.
    #[default]
    Default,
    /// No model: listening by the meters alone.
    Off,
    /// Gemini's newest model that takes audio, with a Gemini API key.
    Gemini,
    /// OpenAI's newest audio model, with an OpenAI API key.
    Openai,
    /// A model on this computer behind an OpenAI-compatible server that takes audio: its address and model.
    Local { base: String, model: String },
    /// A model file on this computer, for Kumi's own model runtime to load (the embeddings model, once fetched).
    File { path: String },
}

/// A choice for a job, in words.
pub fn describe(job: Job, choice: &Choice) -> String {
    match (job, choice) {
        (_, Choice::File { path }) => format!("the model file {path}"),
        (Job::Stems, _) => "Live's own splitter".into(),
        (Job::Transcription, _) => "Live's conversions (drums, melody and harmony to MIDI)".into(),
        (Job::Embeddings, _) => "none yet".into(),
        (Job::Listening, Choice::Default) => {
            "the lookup: a model named in KUMI_LISTENER, else Gemini with a Gemini key, else OpenAI with an OpenAI key".into()
        }
        (Job::Listening, Choice::Off) => "off: the meters alone".into(),
        (Job::Listening, Choice::Gemini) => "Gemini (its newest model that takes audio)".into(),
        (Job::Listening, Choice::Openai) => "OpenAI (its newest audio model)".into(),
        (Job::Listening, Choice::Local { base, model }) => format!("{model} at {base}"),
    }
}

/// A slot: what it holds, and what it held before each swap (the latest last).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub now: Choice,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<Choice>,
}

/// The slots, as kept in `~/.kumi/slots.json`: only the ones swapped from their default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Slots(pub BTreeMap<Job, Slot>);

/// Where the slots are kept, in Kumi's folder.
pub fn file_in(kumi_dir: &Path) -> PathBuf {
    kumi_dir.join("slots.json")
}

impl Slots {
    /// The slots kept in `file`; the defaults when there's no file, or it can't be read.
    pub fn load(file: &Path) -> Slots {
        std::fs::read_to_string(file).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    /// Kept in `file`, written whole to a file beside it and moved into place.
    pub fn save(&self, file: &Path) -> Result<(), String> {
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        let partial = file.with_extension("json.partial");
        std::fs::write(&partial, text + "\n").map_err(|error| error.to_string())?;
        std::fs::rename(&partial, file).map_err(|error| error.to_string())
    }

    pub fn now(&self, job: Job) -> Choice {
        self.0.get(&job).map(|slot| slot.now.clone()).unwrap_or_default()
    }

    /// The model file a slot holds, for Kumi's own model runtime to load (the embeddings model, once fetched); None
    /// while the slot holds anything else.
    pub fn model_file(&self, job: Job) -> Option<PathBuf> {
        match self.now(job) {
            Choice::File { path } => Some(PathBuf::from(path)),
            _ => None,
        }
    }

    /// Swaps a slot to `choice`, keeping what it held to go back to.
    pub fn switch(&mut self, job: Job, choice: Choice) {
        let slot = self.0.entry(job).or_default();
        let was = std::mem::replace(&mut slot.now, choice);
        slot.before.push(was);
        // Ten swaps back is plenty.
        if slot.before.len() > 10 {
            slot.before.remove(0);
        }
    }

    /// Takes the last swap back: what the slot holds again, or None when it was never swapped.
    pub fn back(&mut self, job: Job) -> Option<Choice> {
        let slot = self.0.get_mut(&job)?;
        let was = slot.before.pop()?;
        slot.now = was.clone();
        if slot.now == Choice::Default && slot.before.is_empty() {
            self.0.remove(&job);
        }
        Some(was)
    }
}

/// What a slot is asked to hold.
#[derive(Debug, Clone, PartialEq)]
pub enum Wanted {
    Choice(Choice),
    /// A model file on this computer.
    File(String),
    /// A Hugging Face link, or another address for a model to fetch.
    Link(String),
}

/// What /slots is asked, in plain words.
#[derive(Debug, Clone, PartialEq)]
pub enum Asked {
    Show,
    Swap(Job, Wanted),
    Back(Job),
}

const FILLER: &[&str] = &[
    "use", "using", "for", "the", "to", "with", "set", "swap", "switch", "change", "a", "an", "model", "models", "slot", "slots", "please",
    "on", "as", "it", "its", "my", "own", "go", "put", "make", "and", "of", "from", "in", "by",
];
const BACK: &[&str] = &["back", "revert", "undo", "previous", "restore"];
const SHOW: &[&str] = &["show", "list", "status", "which", "what"];
const FILE_ENDINGS: &[&str] = &[".onnx", ".gguf", ".safetensors", ".bin", ".pt", ".pth", ".ckpt", ".tflite", ".mlmodel", ".mlpackage"];

fn job_word(word: &str) -> Option<Job> {
    Job::ALL.into_iter().find(|job| job.words().contains(&word))
}

fn choice_word(word: &str) -> Option<Choice> {
    match word {
        "gemini" | "google" => Some(Choice::Gemini),
        "openai" | "gpt" | "chatgpt" => Some(Choice::Openai),
        "off" | "none" | "nothing" | "meters" => Some(Choice::Off),
        "default" | "auto" | "automatic" | "live" | "live's" | "live’s" | "lives" | "ableton" => Some(Choice::Default),
        _ => None,
    }
}

/// What the words ask: show the slots, swap one, or take a swap back.
pub fn parse(words: &str) -> Result<Asked, String> {
    let (mut job, mut wanted, mut back, mut unknown) = (None, None, false, vec![]);
    for raw in words.split_whitespace() {
        let lower = raw.to_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("hf:") || lower.starts_with("hf.co/") {
            let address = raw.trim_end_matches(|c| matches!(c, ',' | ';'));
            let hugging_face = lower.starts_with("hf:") || lower.contains("huggingface.co") || lower.contains("hf.co/");
            wanted = Some(match address.split_once('#') {
                Some((base, model)) if !hugging_face && !model.trim().is_empty() => {
                    Wanted::Choice(Choice::Local { base: base.trim_end_matches('/').into(), model: model.trim().into() })
                }
                _ => Wanted::Link(address.into()),
            });
            continue;
        }
        if raw.starts_with('/')
            || raw.starts_with("~/")
            || raw.starts_with("./")
            || FILE_ENDINGS.iter().any(|ending| lower.ends_with(ending))
        {
            wanted = Some(Wanted::File(raw.into()));
            continue;
        }
        let word = lower.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’');
        if word.is_empty() || FILLER.contains(&word) || SHOW.contains(&word) {
            continue;
        }
        if BACK.contains(&word) {
            back = true;
        } else if let Some(choice) = choice_word(word) {
            wanted = Some(Wanted::Choice(choice));
        } else if let Some(named) = job_word(word) {
            job = Some(named);
        } else {
            unknown.push(word.to_string());
        }
    }
    let job = job.or(match &wanted {
        Some(Wanted::Choice(Choice::Gemini | Choice::Openai | Choice::Local { .. })) => Some(Job::Listening),
        _ => None,
    });
    let jobs = "stems, transcription, listening or embeddings";
    if back {
        return job.map(Asked::Back).ok_or_else(|| format!("Which slot goes back: {jobs}? Such as /slots back listening."));
    }
    if let Some(word) = unknown.first() {
        return Err(format!(
            "Kumi doesn't know “{word}” as a model{}. {}",
            job.map(|job| format!(" for {}", job.name())).unwrap_or_default(),
            takes(job)
        ));
    }
    match (job, wanted) {
        (_, None) => Ok(Asked::Show),
        (Some(job), Some(wanted)) => Ok(Asked::Swap(job, wanted)),
        (None, Some(_)) => Err(format!("Which slot: {jobs}? Such as /slots listening gemini.")),
    }
}

/// What a slot takes, in a sentence.
fn takes(job: Option<Job>) -> String {
    match job {
        Some(Job::Listening) => "Listening takes gemini, openai, off, default, or a model on this computer as <address>#<model> (an OpenAI-compatible server that takes audio).".into(),
        Some(job @ (Job::Stems | Job::Transcription)) => format!("{} uses {} for now: Kumi can't run a model file for it yet.", capitalized(job.name()), describe(job, &Choice::Default)),
        Some(Job::Embeddings) => "Embeddings stay off for now: Kumi can't run a model file for them yet.".into(),
        None => "The slots: stems, transcription, listening and embeddings (/slots shows them).".into(),
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// What a slot would hold for what was asked, or why it can't: only listening has models to choose from today, and
/// Kumi runs no model files itself yet.
pub fn fits(job: Job, wanted: &Wanted) -> Result<Choice, String> {
    match job {
        Job::Listening => match wanted {
            Wanted::Choice(choice) => Ok(choice.clone()),
            Wanted::File(_) | Wanted::Link(_) => Err("Kumi can't run a model file itself yet. To listen with one on this computer, serve it with an OpenAI-compatible server that takes audio (llama.cpp's llama-server takes a Hugging Face link: llama-server -hf <repo>), then give Kumi its address: /slots listening http://127.0.0.1:8080/v1#<model>.".into()),
        },
        Job::Embeddings => match wanted {
            Wanted::Choice(Choice::Default | Choice::Off) => Ok(Choice::Default),
            _ => Err("Kumi can't run an embedding model itself yet (that waits on the model runtime it will ship), so embeddings stay off. When it can, this is where one goes: /slots embeddings <file or Hugging Face link>.".into()),
        },
        Job::Stems | Job::Transcription => match wanted {
            Wanted::Choice(Choice::Default) => Ok(Choice::Default),
            Wanted::Choice(Choice::Off) => Err(format!("{} needs something to do it: {} is the one Kumi has.", capitalized(job.name()), describe(job, &Choice::Default))),
            Wanted::Choice(_) => Err(format!("That model doesn't do {}; {} does.", job.name(), describe(job, &Choice::Default))),
            _ => Err(format!(
                "Kumi can't run a {} model itself yet (that waits on the model runtime it will ship), so {} stays on {}. When it can, this is where one goes: /slots {} <file or Hugging Face link>.",
                if job == Job::Stems { "stem" } else { "transcription" },
                job.name(),
                describe(job, &Choice::Default),
                job.name()
            )),
        },
    }
}

/// The slots, a line each, and how to swap one.
pub fn show(slots: &Slots, env: &HashMap<String, String>) -> (Vec<String>, String) {
    let mut lines: Vec<String> = Job::ALL
        .into_iter()
        .map(|job| {
            let now = slots.now(job);
            let swapped =
                if now == Choice::Default { String::new() } else { format!(" (swapped: /slots back {} takes it back)", job.name()) };
            format!("{} ({}): {}{swapped}", job.name(), job.about(), describe(job, &now))
        })
        .collect();
    if env_wins(env) {
        lines.push("KUMI_LISTENER is set, so it wins over the listening slot while it is.".into());
    }
    (
        lines,
        "Swap one in plain words: /slots listening gemini, /slots listening off, /slots listening http://127.0.0.1:8080/v1#<model>.".into(),
    )
}

/// A Gemini listener for this computer's Gemini key; None when there's no key.
async fn gemini(store: &dyn CredentialStore, env: &HashMap<String, String>, signal: Signal) -> Result<Option<Rc<dyn Listener>>, String> {
    let Some(key) = gemini_key(store, env).await else { return Ok(None) };
    Ok(gemini_listener(GEMINI, &key, signal).await?.map(|listener| Rc::new(listener) as Rc<dyn Listener>))
}

/// An OpenAI listener for this computer's OpenAI API key; None when there's no key.
async fn openai(store: &dyn CredentialStore, env: &HashMap<String, String>, signal: Signal) -> Result<Option<Rc<dyn Listener>>, String> {
    let Some(key) = api_key_for(ProviderId::Openai, store, Some(env)).await.ok().flatten() else { return Ok(None) };
    Ok(openai_listener(&key.key, signal).await?.map(|listener| Rc::new(listener) as Rc<dyn Listener>))
}

fn local(base: &str, model: &str, env: &HashMap<String, String>) -> Rc<dyn Listener> {
    Rc::new(ChatListener {
        base: base.into(),
        key: env.get("KUMI_LISTENER_KEY").cloned(),
        model: model.into(),
        client: reqwest::Client::builder().timeout(std::time::Duration::from_secs(90)).build().unwrap_or_default(),
    })
}

/// The listener a choice names, found now; None when it finds none (Off, or no key).
async fn found(
    choice: &Choice,
    store: &dyn CredentialStore,
    env: &HashMap<String, String>,
    signal: Signal,
) -> Result<Option<Rc<dyn Listener>>, String> {
    match choice {
        // Kumi runs no listening model file itself.
        Choice::Off | Choice::File { .. } => Ok(None),
        Choice::Gemini => gemini(store, env, signal).await,
        Choice::Openai => openai(store, env, signal).await,
        Choice::Local { base, model } => Ok(Some(local(base, model, env))),
        // Gemini first (the best at naming what it hears), then OpenAI's audio models, each by its own key.
        Choice::Default => match gemini(store, env, signal.clone()).await? {
            Some(found) => Ok(Some(found)),
            None => openai(store, env, signal).await,
        },
    }
}

/// The listening model, found as the app finds it: `KUMI_LISTENER` first (`off`, or a model by its address), then the
/// listening slot in `file`. What it finds follows the slot: a swap counts from its next listen. None when there's
/// no model to listen with.
pub async fn listener(
    file: &Path,
    store: Rc<dyn CredentialStore>,
    env: &HashMap<String, String>,
    signal: Signal,
) -> Result<Option<Rc<dyn Listener>>, String> {
    if listening_off(env) {
        return Ok(None);
    }
    if let Some(listener) = listener_from_env(env) {
        return Ok(Some(Rc::new(listener)));
    }
    let choice = Slots::load(file).now(Job::Listening);
    let Some(now) = found(&choice, store.as_ref(), env, signal).await? else { return Ok(None) };
    Ok(Some(Rc::new(Following { file: file.to_path_buf(), store, env: env.clone(), now: RefCell::new((choice, now)) })))
}

/// The listening model a session found, following the slot: when it's swapped to another model, that one listens from
/// the next ask. (A slot swapped to off counts from Kumi's next start: a session's judge keeps the model it found.)
struct Following {
    file: PathBuf,
    store: Rc<dyn CredentialStore>,
    env: HashMap<String, String>,
    now: RefCell<(Choice, Rc<dyn Listener>)>,
}

#[async_trait(?Send)]
impl Listener for Following {
    fn name(&self) -> String {
        self.now.borrow().1.name()
    }
    fn hears_width(&self) -> bool {
        self.now.borrow().1.hears_width()
    }
    async fn ask(&self, wav: &[u8], aim: &str, signal: Signal) -> Result<Answer, String> {
        let wanted = Slots::load(&self.file).now(Job::Listening);
        if wanted != self.now.borrow().0 {
            if let Ok(Some(swapped)) = found(&wanted, self.store.as_ref(), &self.env, signal.clone()).await {
                *self.now.borrow_mut() = (wanted, swapped);
            }
        }
        let listener = self.now.borrow().1.clone();
        listener.ask(wav, aim, signal).await
    }
}

const CLIP_RATE: u32 = 24_000;

/// 1.2 s of a 220 Hz tone at one level: a sine, or bright (its first 40 harmonics, a saw's).
fn tone(bright: bool) -> Vec<f32> {
    let count = CLIP_RATE as usize * 6 / 5;
    let partials = if bright { 40 } else { 1 };
    let raw: Vec<f64> = (0..count)
        .map(|n| {
            let t = n as f64 / CLIP_RATE as f64;
            (1..=partials).map(|k| (2. * PI * 220. * k as f64 * t).sin() / k as f64).sum::<f64>()
        })
        .collect();
    let rms = (raw.iter().map(|sample| sample * sample).sum::<f64>() / count as f64).sqrt();
    let fade = CLIP_RATE as usize / 100;
    raw.iter().enumerate().map(|(n, sample)| (sample * 0.2 / rms * (n.min(count - 1 - n) as f64 / fade as f64).min(1.)) as f32).collect()
}

/// Two takes as one 16-bit mono WAV, 0.8 s of silence between them, as the judge sends them.
fn clip(first: &[f32], second: &[f32]) -> Vec<u8> {
    let pause = vec![0f32; (0.8 * CLIP_RATE as f64) as usize];
    let data: Vec<u8> =
        first.iter().chain(&pause).chain(second).flat_map(|sample| ((sample.clamp(-1., 1.) * 32767.) as i16).to_le_bytes()).collect();
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend(b"RIFF");
    wav.extend((36 + data.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(CLIP_RATE.to_le_bytes());
    wav.extend((CLIP_RATE * 2).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((data.len() as u32).to_le_bytes());
    wav.extend(data);
    wav
}

/// The known clip, as the judge would send it: a plain tone and the same tone bright, in the order asked for.
pub fn known_clip(bright_second: bool) -> Vec<u8> {
    let (plain, bright) = (tone(false), tone(true));
    if bright_second {
        clip(&plain, &bright)
    } else {
        clip(&bright, &plain)
    }
}

/// What the known clip asks.
pub const KNOWN_AIM: &str = "brighter: more upper harmonics, more top end";

/// The quick test: the known clip both ways round, a listener that hears it says the bright take is closer each time.
pub async fn hears_known_clip(listener: &dyn Listener, signal: Signal) -> Result<String, String> {
    let one = listener.ask(&known_clip(true), KNOWN_AIM, signal.clone()).await?;
    let two = listener.ask(&known_clip(false), KNOWN_AIM, signal).await?;
    let said = |answer: &Answer| answer.closer.trim().to_lowercase();
    if said(&one) == "second" && said(&two) == "first" {
        Ok(format!("{} heard the known clip right both ways round", listener.name()))
    } else {
        Err(format!(
            "{} didn't hear the known clip right: asked which of a plain and a bright tone is brighter, it said {} then {}, where the bright one was second then first",
            listener.name(),
            one.closer,
            two.closer
        ))
    }
}

/// The models a slot command reads and checks with.
pub struct SlotsContext {
    pub file: PathBuf,
    pub store: Rc<dyn CredentialStore>,
    pub env: HashMap<String, String>,
}

/// What `/slots` says.
#[derive(Debug, Clone, PartialEq)]
pub enum Said {
    /// The slots, a line each, and how to swap one.
    Slots { lines: Vec<String>, footer: String },
    /// A swap made, or one taken back.
    Done(String),
    /// Why nothing changed.
    Refused(String),
}

/// `/slots` in plain words: shows the slots, swaps one after its check, or takes a swap back. `progress` hears what's
/// under way (a check takes a few seconds).
pub async fn command(words: &str, context: &SlotsContext, progress: &dyn Fn(String), signal: Signal) -> Said {
    let mut slots = Slots::load(&context.file);
    let asked = match parse(words) {
        Ok(asked) => asked,
        Err(why) => return Said::Refused(why),
    };
    match asked {
        Asked::Show => {
            let (lines, footer) = show(&slots, &context.env);
            Said::Slots { lines, footer }
        }
        Asked::Back(job) => match slots.back(job) {
            Some(now) => match slots.save(&context.file) {
                Ok(()) => Said::Done(format!("{} is back on {}.", capitalized(job.name()), describe(job, &now))),
                Err(why) => Said::Refused(format!("Kumi couldn't keep the slots in {}: {why}", context.file.display())),
            },
            None => Said::Refused(format!("{} hasn't been swapped: it's on {}.", capitalized(job.name()), describe(job, &slots.now(job)))),
        },
        Asked::Swap(job, wanted) => {
            let choice = match fits(job, &wanted) {
                Ok(choice) => choice,
                Err(why) => return Said::Refused(why),
            };
            let was = slots.now(job);
            if choice == was {
                return Said::Refused(format!("{} is already on {}.", capitalized(job.name()), describe(job, &choice)));
            }
            let stays = |why: String| Said::Refused(format!("{} stays on {}: {why}", capitalized(job.name()), describe(job, &was)));
            // A new listening model is tried on the known clip first.
            let heard = if job == Job::Listening && matches!(choice, Choice::Gemini | Choice::Openai | Choice::Local { .. }) {
                progress(format!("Trying {} on a known clip…", describe(job, &choice)));
                let listener = match found(&choice, context.store.as_ref(), &context.env, signal.clone()).await {
                    Ok(Some(listener)) => listener,
                    Ok(None) => return stays(missing_key(&choice)),
                    Err(why) => return stays(format!("Kumi couldn't reach it ({why}).")),
                };
                match hears_known_clip(listener.as_ref(), signal).await {
                    Ok(heard) => Some(heard),
                    Err(why) => return stays(format!("{why}.")),
                }
            } else {
                None
            };
            slots.switch(job, choice.clone());
            if let Err(why) = slots.save(&context.file) {
                return Said::Refused(format!("Kumi couldn't keep the slots in {}: {why}", context.file.display()));
            }
            let mut said = format!("{} now uses {}", capitalized(job.name()), describe(job, &choice));
            if let Some(heard) = heard {
                said.push_str(&format!(": {heard}"));
            }
            said.push('.');
            if job == Job::Listening && choice == Choice::Off {
                said.push_str(" A session that already listens with a model keeps it until Kumi starts again.");
            } else if job == Job::Listening {
                said.push_str(
                    " The judge listens with it from its next listen (within ten minutes, when this session had found no listening model).",
                );
            }
            if job == Job::Listening && env_wins(&context.env) {
                said.push_str(" KUMI_LISTENER is set, though, and wins over the slot while it is.");
            }
            said.push_str(&format!(" /slots back {} takes it back.", job.name()));
            Said::Done(said)
        }
    }
}

fn env_wins(env: &HashMap<String, String>) -> bool {
    env.get("KUMI_LISTENER").is_some_and(|value| !value.trim().is_empty())
}

/// Why a choice found no model.
fn missing_key(choice: &Choice) -> String {
    match choice {
        Choice::Gemini => "there's no Gemini API key on this computer: set GEMINI_API_KEY (or GOOGLE_API_KEY), then try again.".into(),
        Choice::Openai => "there's no OpenAI API key (a ChatGPT sign-in doesn't reach OpenAI's audio models): set OPENAI_API_KEY or run kumi login openai, then try again.".into(),
        _ => "it found no model.".into(),
    }
}
