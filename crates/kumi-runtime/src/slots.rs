//! Model slots: which model does each of Kumi's listening jobs. One slot per job, each with a permissive default:
//! stems with Live's own splitter, transcription with Live's conversions, listening with the lookup (a model named in
//! `KUMI_LISTENER`, else Gemini with a Gemini key, else OpenAI with an OpenAI key), and embeddings with Kumi's own
//! models. A swap is asked for in plain words, or with a model file or a Hugging Face link (fetched over https). A new
//! model is tried first (a listening model on a known clip, an embedding model on two known tones) and switched to
//! only when it passes; every swap is said, and `/slots back` takes it back. A slots file Kumi can't read turns
//! listening off rather than guess, and is never written over.

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
        (Job::Transcription, _) => {
            "Live's conversions (drums, melody and harmony to MIDI), and Basic Pitch for a pitched part in a file that isn't in Live".into()
        }
        (Job::Embeddings, Choice::Off) => "off: no embeddings".into(),
        (Job::Embeddings, _) => {
            "Kumi's own: LAION-CLAP's music model for style and AFx-Rep for effects, fetched from Kumi's models release the first time they're needed".into()
        }
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

/// The slots as kept in Kumi's own folder (KUMI_HOME, or ~/.kumi); listening off when the file can't be read.
pub fn kept() -> Slots {
    let models = crate::models::dir();
    Slots::load(&file_in(models.parent().unwrap_or(&models)))
}

impl Slots {
    /// The slots kept in `file`: the defaults when there's no file; why, when it's there and can't be read (a newer
    /// Kumi's, say, or a hand edit).
    pub fn read(file: &Path) -> Result<Slots, String> {
        let unreadable = |why: String| format!("Kumi can't read the model slots in {} ({why})", file.display());
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Slots::default()),
            Err(error) => return Err(unreadable(error.to_string())),
        };
        serde_json::from_str(&text).map_err(|error| unreadable(error.to_string()))
    }

    /// The slots kept in `file`, failing closed: when it's there and can't be read, listening is off (nothing is sent)
    /// and the rest are on their defaults.
    pub fn load(file: &Path) -> Slots {
        Slots::read(file).unwrap_or_else(|_| Slots::closed())
    }

    /// What a slots file Kumi can't read leaves: listening off.
    pub fn closed() -> Slots {
        Slots([(Job::Listening, Slot { now: Choice::Off, before: vec![] })].into())
    }

    /// Kept in `file`, written whole to a file beside it and moved into place. A file there that Kumi can't read isn't
    /// written over: it's copied beside first, and where to is given back.
    pub fn save(&self, file: &Path) -> Result<Option<PathBuf>, String> {
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        let aside = match Slots::read(file) {
            Ok(_) => None,
            Err(why) => Some(set_aside(file, &why)?),
        };
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        let partial = file.with_extension("json.partial");
        std::fs::write(&partial, text + "\n").map_err(|error| error.to_string())?;
        std::fs::rename(&partial, file).map_err(|error| error.to_string())?;
        Ok(aside)
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

    /// Whether any slot holds `choice`, now or to go back to.
    fn holds(&self, choice: &Choice) -> bool {
        self.0.values().any(|slot| slot.now == *choice || slot.before.contains(choice))
    }
}

/// A slots file Kumi can't read, copied beside it under the first free name (`slots.json.unreadable`): where.
fn set_aside(file: &Path, why: &str) -> Result<PathBuf, String> {
    let aside = (1..100)
        .map(|n| file.with_extension(if n == 1 { "json.unreadable".into() } else { format!("json.unreadable-{n}") }))
        .find(|aside| !aside.exists())
        .ok_or_else(|| format!("{why}, and it has no free name beside it to keep it under, so it isn't written over"))?;
    std::fs::copy(file, &aside)
        .map_err(|error| format!("{why}, and it couldn't be copied to {} ({error}), so it isn't written over", aside.display()))?;
    Ok(aside)
}

/// A model file the producer named, by its full path: `~` is their home, a relative path is from where Kumi was
/// started. Only a file that's there passes.
fn model_path(path: &str, env: &HashMap<String, String>) -> Result<PathBuf, String> {
    let home = || {
        ["HOME", "USERPROFILE"]
            .iter()
            .find_map(|name| env.get(*name).filter(|home| !home.is_empty()).map(PathBuf::from))
            .or_else(home::home_dir)
    };
    let named = match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => home().ok_or("Kumi can't tell where your home folder is: give the model file's full path.")?.join(rest),
        None => PathBuf::from(path),
    };
    let full = match std::fs::canonicalize(&named) {
        Ok(full) => full,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(format!("there's no such file: {}.", named.display())),
        Err(error) => return Err(format!("Kumi can't reach {} ({error}).", named.display())),
    };
    if !full.is_file() {
        return Err(format!("{} isn't a model file but a folder: name the file in it.", full.display()));
    }
    // Windows' full form starts \\?\; a plain drive path reads better, and everything takes it.
    Ok(match full.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        Some(plain) if plain.get(1..2) == Some(":") => PathBuf::from(plain),
        _ => full,
    })
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
const QUOTES: &[(char, char)] = &[('"', '"'), ('\'', '\''), ('“', '”'), ('‘', '’')];

/// Whether a word ends as a model file does (a full stop or comma after it aside).
fn model_ending(word: &str) -> bool {
    let word = word.to_lowercase();
    let word = word.trim_end_matches(['.', ',', ';']);
    FILE_ENDINGS.iter().any(|ending| word.ends_with(ending))
}

/// Whether a word starts a path: from the root, home or here, a drive, or a model file's name.
fn starts_path(word: &str) -> bool {
    let bytes = word.as_bytes();
    let drive = bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/');
    ["/", "~/", "~\\", "./", ".\\", "../", "..\\", "\\\\"].iter().any(|start| word.starts_with(start)) || drive || model_ending(word)
}

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

/// What the words ask: show the slots, swap one, or take a swap back. A model file's path may have spaces: in quotes,
/// or running on to the word with its file ending (else to the end of the line).
pub fn parse(words: &str) -> Result<Asked, String> {
    let (mut job, mut wanted, mut back, mut unknown) = (None, None, false, vec![]);
    // Each word, with where it starts.
    let spans: Vec<(usize, &str)> = words.split_whitespace().map(|word| (word.as_ptr() as usize - words.as_ptr() as usize, word)).collect();
    let mut next = 0;
    while next < spans.len() {
        let (start, raw) = spans[next];
        next += 1;
        let lower = raw.to_lowercase();
        // A path in quotes, spaces and all.
        if let Some((open, close)) = QUOTES.iter().find(|(open, _)| raw.starts_with(*open)) {
            let inside = &words[start + open.len_utf8()..];
            if let Some(path) =
                inside.find(*close).map(|end| &inside[..end]).filter(|path| starts_path(path.trim()) && !path.contains("://"))
            {
                wanted = Some(Wanted::File(path.trim().into()));
                let after = start + open.len_utf8() + path.len() + close.len_utf8();
                while next < spans.len() && spans[next].0 < after {
                    next += 1;
                }
                continue;
            }
        }
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
        if starts_path(raw) {
            // A path with spaces runs on to the word with its file ending, or to the end of the line.
            let last = if model_ending(raw) {
                next - 1
            } else {
                (next..spans.len()).find(|at| model_ending(spans[*at].1)).unwrap_or(spans.len() - 1)
            };
            let end = spans[last].0 + spans[last].1.len();
            wanted = Some(Wanted::File(words[start..end].trim_end_matches(['.', ',', ';']).into()));
            next = last + 1;
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
        Some(Job::Embeddings) => "Embeddings take default (Kumi's own), off, or an ONNX model file or an https link to one: /slots embeddings ~/models/clap.onnx.".into(),
        None => "The slots: stems, transcription, listening and embeddings (/slots shows them).".into(),
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// What a slot would hold for what was asked, or why it can't: listening takes a model by name or address, embeddings
/// an ONNX model file (or a link to one) for Kumi's own runtime, and stems and transcription keep Live's own for now.
pub fn fits(job: Job, wanted: &Wanted) -> Result<Choice, String> {
    match job {
        Job::Listening => match wanted {
            Wanted::Choice(choice) => Ok(choice.clone()),
            Wanted::File(_) | Wanted::Link(_) => Err("Kumi doesn't run a listening model file itself. To listen with one on this computer, serve it with an OpenAI-compatible server that takes audio (llama.cpp's llama-server takes a Hugging Face link: llama-server -hf <repo>), then give Kumi its address: /slots listening http://127.0.0.1:8080/v1#<model>.".into()),
        },
        Job::Embeddings => match wanted {
            Wanted::Choice(choice @ (Choice::Default | Choice::Off)) => Ok(choice.clone()),
            Wanted::Choice(_) => Err("That's a listening model; the embeddings slot takes Kumi's own (default), off, or an ONNX model file or link that takes CLAP's input (a 10 s log-mel, 1001 × 64) and gives an embedding.".into()),
            Wanted::File(path) if path.to_lowercase().ends_with(".onnx") => Ok(Choice::File { path: path.clone() }),
            Wanted::Link(link) if link.to_lowercase().ends_with(".onnx") => Ok(Choice::File { path: link.clone() }),
            _ => Err("Kumi's runtime runs ONNX models: give an .onnx file, or a Hugging Face link to one (…/resolve/main/model.onnx).".into()),
        },
        Job::Stems | Job::Transcription => match wanted {
            Wanted::Choice(Choice::Default) => Ok(Choice::Default),
            Wanted::Choice(Choice::Off) => Err(format!("{} needs something to do it: {} is the one Kumi has.", capitalized(job.name()), describe(job, &Choice::Default))),
            Wanted::Choice(_) => Err(format!("That model doesn't do {}; {} does.", job.name(), describe(job, &Choice::Default))),
            _ => Err(format!(
                "Kumi can't run a {} model itself yet, so {} stays on {}. When it can, this is where one goes: /slots {} <file or Hugging Face link>.",
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
            let swapped = if slots.0.get(&job).is_some_and(|slot| !slot.before.is_empty()) {
                format!(" (swapped: /slots back {} takes it back)", job.name())
            } else {
                String::new()
            };
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
        // Gemini first (the best at naming what it hears), then OpenAI's audio models, each by its own key. A Gemini
        // lookup that fails (a key made for another Google API, say) still lets OpenAI be tried; why it failed is
        // said only when OpenAI has none either.
        Choice::Default => match gemini(store, env, signal.clone()).await {
            Ok(Some(found)) => Ok(Some(found)),
            Ok(None) => openai(store, env, signal).await,
            Err(why) => match openai(store, env, signal).await {
                Ok(Some(found)) => Ok(Some(found)),
                _ => Err(why),
            },
        },
    }
}

/// The listening model, found as the app finds it: `KUMI_LISTENER` first (`off`, or a model by its address), then the
/// listening slot in `file`. What it finds follows the slot: a swap counts from its next listen. None when there's
/// no model to listen with; why, when the slots file can't be read (listening is off then).
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
    let choice = Slots::read(file).map_err(|why| format!("{why}, so listening is off until /slots swaps it again"))?.now(Job::Listening);
    let Some(now) = found(&choice, store.as_ref(), env, signal).await? else { return Ok(None) };
    Ok(Some(Rc::new(Following { file: file.to_path_buf(), store, env: env.clone(), now: RefCell::new((choice, now)) })))
}

/// The listening model a session found, following the slot: swapped to another model, that one listens from the next
/// ask; swapped off (or the slots file can't be read), it's off at once and nothing is sent.
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
    fn off(&self) -> bool {
        Slots::load(&self.file).now(Job::Listening) == Choice::Off
    }
    async fn ask(&self, wav: &[u8], aim: &str, signal: Signal) -> Result<Answer, String> {
        // Only the model the slot names now is asked: off sends nothing, and another is found first.
        let wanted = Slots::load(&self.file).now(Job::Listening);
        if wanted == Choice::Off {
            return Err("listening is off in its slot, so nothing was sent".into());
        }
        if wanted != self.now.borrow().0 {
            let Some(swapped) = found(&wanted, self.store.as_ref(), &self.env, signal.clone()).await? else {
                return Err(format!(
                    "the listening slot now names {}, which found no model, so nothing was sent",
                    describe(Job::Listening, &wanted)
                ));
            };
            *self.now.borrow_mut() = (wanted, swapped);
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
/// under way, and `signal` stops a fetch or a check (a listening model's check has its own minute and a half).
pub async fn command(words: &str, context: &SlotsContext, progress: &dyn Fn(String), signal: Signal) -> Said {
    let asked = match parse(words) {
        Ok(asked) => asked,
        Err(why) => return Said::Refused(why),
    };
    // A slots file Kumi can't read leaves listening off; a swap copies it beside before writing the slots afresh.
    let (mut slots, unreadable) = match Slots::read(&context.file) {
        Ok(slots) => (slots, None),
        Err(why) => (Slots::closed(), Some(why)),
    };
    let keep = |slots: &Slots| {
        slots.save(&context.file).map_err(|why| format!("Kumi couldn't keep the slots in {}: {why}", context.file.display()))
    };
    match asked {
        Asked::Show => {
            let (mut lines, footer) = show(&slots, &context.env);
            if let Some(why) = unreadable {
                lines.insert(0, format!("{why}, so listening is off. A swap here copies that file beside it and writes the slots afresh."));
            }
            Said::Slots { lines, footer }
        }
        Asked::Back(job) => {
            if let Some(why) = unreadable {
                return Said::Refused(format!(
                    "{why}, so there's no swap to take back; listening is off until a swap here writes the slots afresh."
                ));
            }
            match slots.back(job) {
                Some(now) => match keep(&slots) {
                    Ok(_) => Said::Done(format!("{} is back on {}.", capitalized(job.name()), describe(job, &now))),
                    Err(why) => Said::Refused(why),
                },
                None => {
                    Said::Refused(format!("{} hasn't been swapped: it's on {}.", capitalized(job.name()), describe(job, &slots.now(job))))
                }
            }
        }
        Asked::Swap(job, wanted) => {
            let choice = match fits(job, &wanted) {
                Ok(choice) => choice,
                Err(why) => return Said::Refused(why),
            };
            let was = slots.now(job);
            let stays = |why: String| Said::Refused(format!("{} stays on {}: {why}", capitalized(job.name()), describe(job, &was)));
            // A model file by its full path, there; one from a link fetched first (over https, into Kumi's folder).
            let choice = match (choice, &wanted) {
                (Choice::File { .. }, Wanted::Link(link)) => {
                    progress(format!("Fetching {link} (Esc stops it)…"));
                    match crate::listening::embed::fetch_link(link, &signal).await {
                        Ok(fetched) => Choice::File { path: fetched.to_string_lossy().into_owned() },
                        Err(why) => return stays(why),
                    }
                }
                (Choice::File { path }, _) => match model_path(&path, &context.env) {
                    Ok(path) => Choice::File { path: path.to_string_lossy().into_owned() },
                    Err(why) => return stays(why),
                },
                (choice, _) => choice,
            };
            if choice == was {
                return Said::Refused(format!("{} is already on {}.", capitalized(job.name()), describe(job, &choice)));
            }
            // An embedding model is tried on two known tones, which it has to tell apart.
            let tried = if let (Job::Embeddings, Choice::File { path }) = (job, &choice) {
                progress(format!("Trying {path} on two known tones…"));
                let say = |said: &str| progress(said.to_string());
                // The runtime first: one that can't be had says nothing about the model.
                if let Err(why) = crate::models::runtime(&say, &signal).await {
                    return stays(format!("{}.", why.trim_end_matches('.')));
                }
                match crate::listening::embed::tells_tones_apart(Path::new(path), &say, &signal).await {
                    Ok(heard) => Some(heard),
                    Err(why) => {
                        // Kumi's copy of a link that fails goes, so the next try fetches it again.
                        if matches!(wanted, Wanted::Link(_)) && !signal.is_cancelled() && !slots.holds(&choice) {
                            let _ = std::fs::remove_file(path);
                            if let Some(folder) = Path::new(path).parent() {
                                let _ = std::fs::remove_dir(folder);
                            }
                        }
                        return stays(format!("{why}."));
                    }
                }
            } else {
                None
            };
            // A new listening model is tried on the known clip first: one that hasn't answered in a minute and a half
            // won't.
            let heard = if job == Job::Listening && matches!(choice, Choice::Gemini | Choice::Openai | Choice::Local { .. }) {
                progress(format!("Trying {} on a known clip…", describe(job, &choice)));
                let signal = kumi_common::abort::any([signal.clone(), kumi_common::abort::timeout(90_000)]);
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
            // Swapped meanwhile (another /slots while this one was tried): the newer word stands.
            let mut latest = Slots::load(&context.file);
            if latest.0.get(&job) != slots.0.get(&job) {
                return Said::Refused(format!(
                    "{} was swapped to {} while Kumi tried this one, so this swap isn't made: ask again to make it.",
                    capitalized(job.name()),
                    describe(job, &latest.now(job))
                ));
            }
            latest.switch(job, choice.clone());
            let aside = match keep(&latest) {
                Ok(aside) => aside,
                Err(why) => return Said::Refused(why),
            };
            let mut said = format!("{} now uses {}", capitalized(job.name()), describe(job, &choice));
            if let Some(heard) = heard.or(tried) {
                said.push_str(&format!(": {heard}"));
            }
            said.push('.');
            if job == Job::Listening && choice == Choice::Off {
                said.push_str(" Nothing is sent from now on.");
            } else if job == Job::Listening {
                said.push_str(
                    " The judge listens with it from its next listen (within ten minutes, when this session had found no listening model).",
                );
            }
            if job == Job::Listening && env_wins(&context.env) {
                said.push_str(" KUMI_LISTENER is set, though, and wins over the slot while it is.");
            }
            if let Some(aside) = aside {
                said.push_str(&format!(" The slots file Kumi couldn't read is copied to {}.", aside.display()));
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
