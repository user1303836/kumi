//! What the producer can name with `@` in a message: the project's tracks, devices, instruments and clips, and the
//! library's sounds, palettes and techniques, ranked as they type, each an exact [`Reference`].

use std::rc::Rc;

use kumi_common::js::json::stringify;
use kumi_library::search::SoundIndex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::contracts::Reference;

/// What a candidate is, in words any DAW's front end puts in its own (a device is REAPER's FX and Renoise's DSP).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CandidateKind {
    Track,
    Device,
    Instrument,
    Clip,
    Sample,
    Palette,
    Technique,
}

/// One thing the producer can name with `@`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// Its name as the list shows it: "Drums", "Big Hall", "Kick 808.wav".
    pub label: String,
    pub kind: CandidateKind,
    /// Where it is, when that tells it apart: "on Drums", "in Drum Kits/808".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub reference: Reference,
}

/// How well `query` names `name`, both lowercased: None unless the query's letters are all in the name, in order. A
/// prefix beats a word's start, which beats letters scattered, and a run of letters beats the same letters apart.
fn score(name: &str, query: &[char]) -> Option<u32> {
    let (mut score, mut matched, mut prefix) = (0, 0, true);
    let (mut previous, mut run) = (None::<char>, false);
    for (at, letter) in name.chars().enumerate() {
        let Some(wanted) = query.get(matched) else { break };
        if letter == *wanted {
            score += 1;
            if previous.is_none_or(|previous| !previous.is_alphanumeric()) {
                score += 8;
            }
            if run {
                score += 4;
            }
            prefix &= at == matched;
            matched += 1;
            run = true;
        } else {
            run = false;
        }
        previous = Some(letter);
    }
    if matched < query.len() {
        return None;
    }
    if prefix {
        score += 16;
    }
    // Among equals, the shorter name: it's the closer one.
    Some(score * 64 - (name.len() as u32).min(63))
}

/// A list `@` ranks by place without copying it: the short lists, or the library's sounds, which stay in their index
/// until one is among the best.
pub trait Listed {
    fn count(&self) -> usize;
    /// The name at `at`, lowercased as it's matched.
    fn lower(&self, at: usize) -> &str;
    /// What `at` is as the list shows it: made only for the best.
    fn candidate(&self, at: usize) -> Option<Candidate>;
}

/// The short lists, read as the list opens: the project's things, the palettes, the techniques.
#[derive(Debug, Default)]
pub struct Short {
    candidates: Vec<Candidate>,
    lower: Vec<String>,
}

impl Short {
    pub fn new(candidates: Vec<Candidate>) -> Short {
        let lower = candidates.iter().map(|candidate| candidate.label.to_lowercase()).collect();
        Short { candidates, lower }
    }
}

impl Listed for Short {
    fn count(&self) -> usize {
        self.candidates.len()
    }
    fn lower(&self, at: usize) -> &str {
        &self.lower[at]
    }
    fn candidate(&self, at: usize) -> Option<Candidate> {
        self.candidates.get(at).cloned()
    }
}

/// The library's sounds by the names its search shows (a file's, without the extension), lowercased once there.
impl Listed for SoundIndex {
    fn count(&self) -> usize {
        self.entries.len()
    }
    fn lower(&self, at: usize) -> &str {
        self.named(at).map_or("", |(_, lower, _)| lower)
    }
    fn candidate(&self, at: usize) -> Option<Candidate> {
        let (name, _, place) = self.named(at)?;
        let path = self.entries.get(at)?.file.path.clone();
        Some(Candidate {
            label: name.to_owned(),
            kind: CandidateKind::Sample,
            detail: Some(format!("in {place}")),
            reference: Reference::Sample { path, name: name.to_owned() },
        })
    }
}

/// What `@` offers while it's open, ranked as the producer types: the short lists, then a long one (the library's
/// sounds). A query that only grows is matched among the last one's matches (a name with all of `kic` in order has all
/// of `ki`), so each key costs what's left to match rather than the whole library, and only the best are sorted.
pub struct Referable {
    short: Short,
    long: Option<Rc<dyn Listed>>,
    last: Option<(Vec<char>, Vec<usize>)>,
}

impl Referable {
    /// `short` in the order ties keep (the project's first, as its integration orders them), then `long`.
    pub fn new(short: Vec<Candidate>, long: Option<Rc<dyn Listed>>) -> Referable {
        Referable { short: Short::new(short), long, last: None }
    }

    fn lower(&self, at: usize) -> &str {
        match at.checked_sub(self.short.count()) {
            None => self.short.lower(at),
            Some(at) => self.long.as_ref().map_or("", |long| long.lower(at)),
        }
    }

    fn candidate(&self, at: usize) -> Option<Candidate> {
        match at.checked_sub(self.short.count()) {
            None => self.short.candidate(at),
            Some(at) => self.long.as_ref().and_then(|long| long.candidate(at)),
        }
    }

    /// The best `limit` for `query`, best first. Equal scores keep the lists' order, so the project's come before the
    /// library's; an empty query keeps it all along.
    pub fn rank(&mut self, query: &str, limit: usize) -> Vec<Candidate> {
        let query: Vec<char> = query.to_lowercase().chars().collect();
        let total = self.short.count() + self.long.as_ref().map_or(0, |long| long.count());
        if query.is_empty() {
            self.last = None;
            return (0..total.min(limit)).filter_map(|at| self.candidate(at)).collect();
        }
        let narrowed = self.last.take().filter(|(last, _)| query.starts_with(last)).map(|(_, matched)| matched);
        let scored = |at: usize| score(self.lower(at), &query).map(|score| (score, at));
        let mut found: Vec<(u32, usize)> = match narrowed {
            Some(matched) => matched.into_iter().filter_map(scored).collect(),
            None => (0..total).filter_map(scored).collect(),
        };
        self.last = Some((query, found.iter().map(|(_, at)| *at).collect()));
        let order = |a: &(u32, usize), b: &(u32, usize)| b.0.cmp(&a.0).then(a.1.cmp(&b.1));
        if found.len() > limit {
            found.select_nth_unstable_by(limit, order);
            found.truncate(limit);
        }
        found.sort_unstable_by(order);
        found.into_iter().filter_map(|(_, at)| self.candidate(at)).collect()
    }
}

/// The turn's context with the library's references in it, beside the project's under the same `"references"` key:
/// their names come from the producer's files and lists, data like the rest of the observation. A context that isn't
/// an object (a test's) has them after it.
pub fn with_references(context: &str, references: Vec<Value>) -> String {
    if references.is_empty() {
        return context.to_owned();
    }
    match serde_json::from_str::<Value>(context) {
        Ok(Value::Object(mut object)) => {
            match object.get_mut("references").and_then(Value::as_array_mut) {
                Some(listed) => listed.extend(references),
                None => {
                    object.insert("references".into(), Value::Array(references));
                }
            }
            stringify(&Value::Object(object))
        }
        _ => format!("{context}\n{}", stringify(&json!({ "references": references }))),
    }
}
