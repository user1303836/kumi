//! Timed captions from YouTube json3, WebVTT and SRT.
use kumi_common::js::{
    number::to_string,
    string::{head, trim, utf16_len},
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptLine {
    pub at: f64,
    pub text: String,
}
pub fn parse_time(value: &Value) -> Option<f64> {
    if let Some(n) = value.as_f64() {
        return (n.is_finite() && n >= 0.0).then_some(n);
    }
    let text = trim(value.as_str()?);
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+(\.[0-9]+)?$").unwrap());
    if NUMBER.is_match(text) {
        return text.parse().ok();
    }
    static TIME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:([0-9]+):)?([0-9]{1,2}):([0-9]{1,2}(?:\.[0-9]+)?)$").unwrap());
    let c = TIME.captures(text)?;
    Some(
        c.get(1).map_or(0.0, |s| s.as_str().parse::<f64>().unwrap_or(f64::INFINITY)) * 3600.0
            + c[2].parse::<f64>().ok()? * 60.0
            + c[3].parse::<f64>().ok()?,
    )
}
pub fn format_time(seconds: f64) -> String {
    let whole = seconds.floor().max(0.0);
    let hours = (whole / 3600.0).floor();
    let minutes = ((whole % 3600.0) / 60.0).floor();
    let secs = whole % 60.0;
    let sec = to_string(secs);
    if hours != 0.0 {
        format!("{}:{:0>2}:{:0>2}", to_string(hours), to_string(minutes), sec)
    } else {
        format!("{}:{:0>2}", to_string(minutes), sec)
    }
}
fn clean(text: &str) -> String {
    static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
    static SOUNDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]{1,30}\]").unwrap());
    static CONTROLS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\x00-\x1f\x7f]").unwrap());
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    let text = TAGS
        .replace_all(text, "")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    trim(&SPACES.replace_all(&CONTROLS.replace_all(&SOUNDS.replace_all(&text, " "), " "), " ")).into()
}
pub fn parse_captions(text: &str, format: &str) -> Vec<Cue> {
    let mut cues = Vec::new();
    if format == "json3" {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return cues;
        };
        for event in value["events"].as_array().into_iter().flatten() {
            let Some(segments) = event["segs"].as_array() else {
                continue;
            };
            let Some(ms) = event["tStartMs"].as_f64() else {
                continue;
            };
            let line = clean(&segments.iter().map(|s| s["utf8"].as_str().unwrap_or("")).collect::<String>());
            if line.is_empty() {
                continue;
            }
            let start = ms / 1000.0;
            cues.push(Cue { start, end: start + event["dDurationMs"].as_f64().unwrap_or(0.0) / 1000.0, text: line });
        }
    } else {
        static TIMING: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"([0-9]{1,2}:)?([0-9]{1,2}):([0-9]{2})[.,]([0-9]{3})\s*-->\s*([0-9]{1,2}:)?([0-9]{1,2}):([0-9]{2})[.,]([0-9]{3})")
                .unwrap()
        });
        static BLOCKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{2,}").unwrap());
        let mut last = String::new();
        let text = text.replace('\r', "");
        for block in BLOCKS.split(&text) {
            let lines: Vec<_> = block.split('\n').collect();
            let Some((at, c)) = lines.iter().enumerate().find_map(|(at, line)| TIMING.captures(line).map(|c| (at, c))) else {
                continue;
            };
            let seconds = |h: usize, m: usize, s: usize, ms: usize| {
                c.get(h).map_or(0.0, |s| s.as_str().trim_end_matches(':').parse::<f64>().unwrap()) * 3600.0
                    + c[m].parse::<f64>().unwrap() * 60.0
                    + c[s].parse::<f64>().unwrap()
                    + c[ms].parse::<f64>().unwrap() / 1000.0
            };
            let start = seconds(1, 2, 3, 4);
            let end = seconds(5, 6, 7, 8);
            for raw in &lines[at + 1..] {
                let line = clean(raw);
                if line.is_empty() || line == last {
                    continue;
                }
                last = line.clone();
                cues.push(Cue { start, end, text: line });
            }
        }
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    cues
}
#[derive(Clone, Copy, Default)]
pub struct TranscriptOptions {
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub chars: Option<usize>,
}
pub fn transcript_lines(cues: &[Cue], options: TranscriptOptions) -> Vec<TranscriptLine> {
    let from = options.from.unwrap_or(0.0);
    let to = options.to.unwrap_or(f64::INFINITY);
    let chars = options.chars.unwrap_or(220);
    let mut lines = Vec::new();
    let mut current: Option<(TranscriptLine, f64)> = None;
    for cue in cues {
        if cue.end < from || cue.start > to {
            continue;
        }
        let start = current.as_ref().is_none_or(|(c, end)| {
            cue.start - end > 2.5
                || (c.text.ends_with(['.', '!', '?']) && utf16_len(&c.text) as f64 > chars as f64 / 2.0)
                || utf16_len(&c.text) + utf16_len(&cue.text) > chars
        });
        if start {
            if let Some((c, _)) = current.take() {
                lines.push(c);
            }
            current = Some((TranscriptLine { at: cue.start, text: cue.text.clone() }, cue.end));
        } else if let Some((c, end)) = &mut current {
            c.text.push(' ');
            c.text.push_str(&cue.text);
            *end = end.max(cue.end);
        }
    }
    if let Some((c, _)) = current {
        lines.push(c);
    }
    lines
}
pub fn said_around(cues: &[Cue], at: f64, span: Option<f64>) -> String {
    let span = span.unwrap_or(4.0);
    head(
        &cues.iter().filter(|c| c.end >= at - span && c.start <= at + span / 2.0).map(|c| c.text.as_str()).collect::<Vec<_>>().join(" "),
        240,
    )
}
