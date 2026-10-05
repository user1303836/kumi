//! Moments whose narration names devices/settings/values, chapter starts, or spread through.
use super::captions::Cue;
use kumi_common::js::number::round;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub start: f64,
    pub title: String,
}
#[derive(Clone, Default)]
pub struct MomentOptions {
    pub from: f64,
    pub to: f64,
    pub count: f64,
    pub chapters: Vec<Chapter>,
}
static WORDS: LazyLock<(Vec<String>, Vec<Regex>)> = LazyLock::new(|| {
    let v: serde_json::Value = serde_json::from_str(include_str!("moment-words.json")).unwrap();
    (
        v["devices"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().into()).collect(),
        v["settings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| Regex::new(&format!(r"(?-u:\b){}(?-u:\b)", regex::escape(s.as_str().unwrap()))).unwrap())
            .collect(),
    )
});
static POINTING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)(like (this|that|so)|this (knob|one|here|parameter|setting)|right (here|there)|over here|about (here|there)|all the way|set (it|this|that) to|turn (it|this|that)|bring (it|this|that)|drag|dial|map (it|this|that)|you can see|as you can see|looks like)(?-u:\b)").unwrap()
});
static VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)[0-9]+(\.[0-9]+)?\s?(%|db|hz|khz|ms|s(?-u:\b)|semitones?|cents?|bars?|beats?)|(?-u:\b)1/(4|8|16|32)(?-u:\b)|(?-u:\b)[0-9]{2,3}\s?(percent|bpm)(?-u:\b)").unwrap()
});
fn score(text: &str) -> usize {
    let lower = text.to_lowercase();
    WORDS.0.iter().filter(|s| lower.contains(s.as_str())).count() * 3
        + WORDS.1.iter().filter(|s| s.is_match(&lower)).count() * 2
        + usize::from(POINTING.is_match(text)) * 3
        + usize::from(VALUE.is_match(text)) * 2
}
pub fn choose_moments(cues: &[Cue], options: MomentOptions) -> Vec<f64> {
    let (from, to) = (options.from, options.to);
    let count = options.count.floor().clamp(0.0, 16.0) as usize;
    if count == 0 || !(to > from) {
        return Vec::new();
    }
    let span = to - from;
    let in_range = |time: f64| (to - 0.5).min((from + 0.5).max(time));
    let mut candidates: Vec<_> = cues
        .iter()
        .filter(|c| c.end >= from && c.start <= to)
        .map(|c| (in_range(c.start + 2.5_f64.min(1.0_f64.max((c.end - c.start) / 2.0))), score(&c.text)))
        .filter(|(_, points)| *points > 0)
        .collect();
    for chapter in options.chapters {
        if chapter.start >= from && chapter.start < to {
            candidates.push((in_range(chapter.start + 3.0), 4));
        }
    }
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.total_cmp(&b.0)));
    let gap = 4.0_f64.max(span / (count as f64 * 2.5));
    let mut chosen: Vec<f64> = Vec::new();
    for (at, _) in candidates {
        if chosen.len() >= count {
            break;
        }
        if chosen.iter().all(|t| (*t - at).abs() >= gap) {
            chosen.push(at);
        }
    }
    for index in 0..count * 3 {
        if chosen.len() >= count {
            break;
        }
        let at = in_range(from + span * (0.08 + 0.84 * ((index as f64 + 0.5) / (count as f64 * 1.5))));
        if chosen.iter().all(|t| (*t - at).abs() >= gap / 2.0) {
            chosen.push(at);
        }
    }
    chosen.sort_by(f64::total_cmp);
    chosen.into_iter().map(|t| round(t * 10.0) / 10.0).collect()
}
