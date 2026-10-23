//! A DAW's manual, kept on this computer and searched by section with cited passages: what every manual Kumi reads
//! shares. A DAW's own source gives the pages and how they read into sections, its own stop words and its words (Live's
//! in `ableton::manual`, Logic Pro's in `logic_manual`). Here the copy is kept (read once into the library folder, read
//! again in the background after 45 days while the old copy answers, concurrent reads sharing one fetch) and searched
//! (stems, BM25 over each section's title, where it sits and its text, and the passages that answer).
use crate::library::{
    store::{read_json, write_json},
    tools::{tell, OnEvent},
};
use crate::web::net::{create_web_client, decode_text, status_words, WebClient, WebClientOptions, WebError, WebFailure, WebRequest};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use indexmap::IndexSet;
use kumi_common::{
    abort::Signal,
    js::{
        number::round,
        string::{head, trim, utf16_len},
    },
    time::now_ms,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    cell::{OnceCell, RefCell},
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    rc::Rc,
    sync::LazyLock,
};

/// How long a kept copy is fresh: an older one still answers while a new one is read in the background.
pub const FRESH_MS: i64 = 45 * 24 * 60 * 60000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManualSection {
    pub number: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<String>,
    pub chapter: String,
    pub url: String,
    pub text: String,
}

/// A manual as its file keeps it: its sections, when they were read, and, for a manual whose pages are read one at a
/// time as answers need them (Logic Pro's guide), the pages read since, by their number, each read into its sections.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kept {
    pub version: u32,
    pub fetched_at: i64,
    pub sections: Vec<ManualSection>,
    #[serde(default, skip_serializing_if = "no_pages")]
    pub pages: RefCell<BTreeMap<String, Vec<ManualSection>>>,
    #[serde(skip)]
    prepared: OnceCell<Prepared>,
}

fn no_pages(pages: &RefCell<BTreeMap<String, Vec<ManualSection>>>) -> bool {
    pages.borrow().is_empty()
}

impl Kept {
    /// The sections' term counts for a search, counted the first time one asks.
    pub fn prepared(&self, own: &[&str]) -> &Prepared {
        self.prepared.get_or_init(|| prepare(&self.sections, own))
    }
}

#[derive(Clone)]
pub struct Prepared {
    counts: Vec<HashMap<String, usize>>,
    lengths: Vec<usize>,
    average: f64,
    frequency: HashMap<String, usize>,
}

pub fn collapse_space(text: &str) -> String {
    static SPACE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[\t\n\x0B\x0C\r \u{00A0}\u{FEFF}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}]+").unwrap()
    });
    SPACE.replace_all(text, " ").into_owned()
}

/// Words no search goes by, in any manual; a source adds its own (its DAW's name).
const STOP: &[&str] = &[
    "a", "an", "the", "and", "or", "of", "to", "in", "on", "for", "with", "how", "do", "does", "i", "my", "can", "is", "it", "what",
    "when", "where", "why", "which", "you", "your", "me", "make", "use", "using", "get", "be", "are", "this", "that", "from", "by", "as",
    "at", "into", "so", "there", "way",
];

/// A text's words as a search takes them: lower case, two letters or more, without the stop words and the source's
/// `own`, their commonest endings taken off.
pub fn stems(text: &str, own: &[&str]) -> Vec<String> {
    text.to_lowercase()
        .replace(['’', '\''], "")
        .split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '#')
        .filter(|word| word.len() > 1 && !STOP.contains(word) && !own.contains(word))
        .map(|word| {
            {
                if word.len() > 5 && word.ends_with("ing") {
                    &word[..word.len() - 3]
                } else if word.len() > 4 && word.ends_with("ed") {
                    &word[..word.len() - 2]
                } else if word.len() > 3 && word.ends_with('s') && !word.ends_with("ss") {
                    &word[..word.len() - 1]
                } else {
                    word
                }
            }
            .to_owned()
        })
        .collect()
}

pub fn prepare(sections: &[ManualSection], own: &[&str]) -> Prepared {
    let mut frequency = HashMap::new();
    let counts: Vec<_> = sections
        .iter()
        .map(|section| {
            let mut counted = HashMap::new();
            let within = section.within.as_deref().unwrap_or("");
            for word in stems(&format!("{} {} {} {within} {within} {}", section.title, section.title, section.title, section.chapter), own)
                .into_iter()
                .chain(stems(&section.text, own))
            {
                *counted.entry(word).or_insert(0) += 1;
            }
            for word in counted.keys() {
                *frequency.entry(word.clone()).or_insert(0) += 1;
            }
            counted
        })
        .collect();
    let lengths: Vec<usize> = counts.iter().map(|c| c.values().sum()).collect();
    Prepared { average: lengths.iter().sum::<usize>() as f64 / lengths.len().max(1) as f64, counts, lengths, frequency }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualHit {
    pub section: ManualSection,
    pub passages: Vec<String>,
    pub score: f64,
}

/// The sections that best answer `question`, best first, at most `limit`: BM25 (k1 1.2, b 0.75) over each section's
/// title (three times), where it sits (twice), its chapter and its text, half as much again for the question's words
/// in a row; each with up to three of its paragraphs that hold the question's words, in their order.
pub fn search(sections: &[ManualSection], prepared: &Prepared, question: &str, limit: usize, own: &[&str]) -> Vec<ManualHit> {
    let terms: IndexSet<_> = stems(question, own).into_iter().collect();
    if terms.is_empty() {
        return vec![];
    }
    let phrase = question
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == ' ' { c } else { ' ' })
        .collect::<String>();
    let phrase = collapse_space(&phrase);
    let phrase = trim(&phrase);
    let mut scored: Vec<_> = prepared
        .counts
        .iter()
        .enumerate()
        .filter_map(|(index, counted)| {
            let mut score = 0.;
            for term in &terms {
                let count = *counted.get(term).unwrap_or(&0) as f64;
                if count == 0. {
                    continue;
                }
                let documents = prepared.frequency[term] as f64;
                let idf = (1. + (sections.len() as f64 - documents + 0.5) / (documents + 0.5)).ln();
                score += idf * count * 2.2 / (count + 1.2 * (0.25 + 0.75 * prepared.lengths[index] as f64 / prepared.average));
            }
            let section = &sections[index];
            if score > 0. && phrase.len() > 6 && format!("{} {}", section.title, section.text).to_lowercase().contains(phrase) {
                score *= 1.5;
            }
            (score > 0.).then_some((index, score))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(limit);
    static PARAGRAPH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{2,}").unwrap());
    scored
        .into_iter()
        .map(|(index, score)| {
            let section = &sections[index];
            let paragraphs: Vec<_> = PARAGRAPH.split(&section.text).filter(|p| !trim(p).is_empty()).collect();
            let mut ranked: Vec<_> = paragraphs
                .iter()
                .enumerate()
                .filter_map(|(at, p)| {
                    let words: std::collections::HashSet<_> = stems(p, own).into_iter().collect();
                    let hits = terms.iter().filter(|t| words.contains(t.as_str())).count();
                    (hits > 0).then_some((at, hits))
                })
                .collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            ranked.truncate(3);
            ranked.sort_by_key(|r| r.0);
            let chosen = if ranked.is_empty() {
                paragraphs.into_iter().take(1).collect::<Vec<_>>()
            } else {
                ranked.into_iter().map(|r| paragraphs[r.0]).collect()
            };
            let passages = chosen.into_iter().map(|p| if utf16_len(p) > 900 { format!("{}…", head(p, 900)) } else { p.into() }).collect();
            ManualHit { section: section.clone(), passages, score: round(score * 100.) / 100. }
        })
        .collect()
}

/// One of a manual's pages, 4 MiB at most; `site` and `manual` say whose, when it answers with an error ("Ableton's
/// site answered 404 Not Found for the manual.").
pub async fn page(client: &dyn WebClient, signal: Signal, url: &str, site: &str, manual: &str) -> Result<String, WebFailure> {
    let response = client.fetch(url, WebRequest { signal: Some(signal), max_bytes: Some(4 * 1024 * 1024), ..Default::default() }).await?;
    if response.status >= 400 {
        return Err(
            WebError::with_status(format!("{site} answered {} for {manual}.", status_words(response.status)), response.status).into()
        );
    }
    Ok(decode_text(&response.body, response.charset.as_deref()))
}

#[derive(Clone, Default)]
pub struct ManualOptions {
    pub dir: String,
    pub client: Option<Rc<dyn WebClient>>,
    pub on_event: Option<OnEvent>,
    pub base: Option<String>,
    pub now: Option<Rc<dyn Fn() -> i64>>,
}

impl ManualOptions {
    /// The client a manual's pages are read with: the one given, else a new one.
    pub fn client(&self) -> Rc<dyn WebClient> {
        self.client.clone().unwrap_or_else(|| create_web_client(WebClientOptions::default()))
    }
}

/// How a source reads its manual's sections: with a client, a signal, and the address to start from when it isn't
/// the source's own (a test's site).
pub type Fetch = Rc<dyn Fn(Rc<dyn WebClient>, Signal, Option<String>) -> LocalBoxFuture<'static, Result<Vec<ManualSection>, WebFailure>>>;

type Reading = Shared<LocalBoxFuture<'static, Result<Rc<Kept>, WebFailure>>>;

#[derive(Default)]
struct KeeperState {
    held: Option<Rc<Kept>>,
    reading: Option<Reading>,
}

/// A source's copy of its manual, kept in the library folder in a file of its own.
pub struct Keeper {
    options: ManualOptions,
    file: PathBuf,
    version: u32,
    first: &'static str,
    fetch: Fetch,
    state: Rc<RefCell<KeeperState>>,
}

impl Keeper {
    /// The copy `file` keeps in the library folder (`options.dir`), as `version` of its source keeps it; `first` is
    /// what the producer is told while it's read the first time.
    pub fn new(options: ManualOptions, file: &str, version: u32, first: &'static str, fetch: Fetch) -> Keeper {
        Keeper {
            file: PathBuf::from(&options.dir).join(file),
            options,
            version,
            first,
            fetch,
            state: Rc::new(RefCell::new(KeeperState::default())),
        }
    }

    pub fn options(&self) -> &ManualOptions {
        &self.options
    }

    pub fn now(&self) -> i64 {
        self.options.now.as_ref().map(|now| now()).unwrap_or_else(now_ms)
    }

    fn read(&self, signal: Signal) -> Reading {
        if let Some(reading) = &self.state.borrow().reading {
            return reading.clone();
        }
        let options = self.options.clone();
        let file = self.file.clone();
        let (version, fetch) = (self.version, self.fetch.clone());
        let state = Rc::downgrade(&self.state);
        let reading = async move {
            let result = async {
                let sections = fetch(options.client(), signal, options.base.clone()).await?;
                let kept = Rc::new(Kept {
                    version,
                    fetched_at: options.now.map(|now| now()).unwrap_or_else(now_ms),
                    sections,
                    pages: RefCell::default(),
                    prepared: OnceCell::new(),
                });
                let _ = write_json(&file, kept.as_ref()).await;
                Ok::<_, WebFailure>(kept)
            }
            .await;
            if let Some(state) = state.upgrade() {
                let mut state = state.borrow_mut();
                state.reading = None;
                if let Ok(kept) = &result {
                    state.held = Some(kept.clone());
                }
            }
            result
        }
        .boxed_local()
        .shared();
        self.state.borrow_mut().reading = Some(reading.clone());
        reading
    }

    /// The manual: the copy held, else the one kept in its file, else read now (the first time, the producer is told);
    /// a copy older than 45 days answers while a new one is read in the background.
    pub async fn manual(&self, signal: Signal) -> Result<Rc<Kept>, WebFailure> {
        if self.state.borrow().held.is_none() {
            let version = self.version;
            let kept = read_json::<Kept>(&self.file).await.filter(|k| k.version == version && !k.sections.is_empty()).map(Rc::new);
            self.state.borrow_mut().held = kept;
        }
        let held = self.state.borrow().held.clone();
        if let Some(held) = held {
            if self.now() - held.fetched_at > FRESH_MS && self.state.borrow().reading.is_none() {
                let read = self.read(kumi_common::abort::timeout(120000));
                tokio::task::spawn_local(async move {
                    let _ = read.await;
                });
            }
            return Ok(held);
        }
        tell(&self.options.on_event, self.first.into());
        self.read(signal).await
    }

    /// Writes `kept` to the file again, with the pages read since: what a later session finds. A copy read again
    /// meanwhile is the file's already, and stays.
    pub async fn keep(&self, kept: &Kept) {
        if self.state.borrow().held.as_ref().is_some_and(|held| std::ptr::eq(held.as_ref(), kept)) {
            let _ = write_json(&self.file, kept).await;
        }
    }
}
