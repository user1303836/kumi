//! Place a parameter value using the text the DAW shows across its range.
use fancy_regex::Regex;
use kumi_common::js::{number, string::trim};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayReading {
    pub value: f64,
    pub unit: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayMap {
    pub min: f64,
    pub max: f64,
    pub grid: Vec<(f64, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DisplayTarget {
    Number(f64),
    Text(String),
}
impl From<f64> for DisplayTarget {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}
impl From<&str> for DisplayTarget {
    fn from(value: &str) -> Self {
        Self::Text(value.into())
    }
}
impl From<String> for DisplayTarget {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
fn regex(source: &str) -> Regex {
    Regex::new(
        &source
            .replace(r"\s", r"[\u0009-\u000d\u0020\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]")
            .replace(r"\d", "[0-9]"),
    )
    .unwrap()
}
pub fn parse_display(text: &str) -> Option<DisplayReading> {
    static INF: LazyLock<Regex> = LazyLock::new(|| regex(r"^-?\s*inf"));
    static RATIO: LazyLock<Regex> = LazyLock::new(|| regex(r"^(-?\d+(?:\.\d+)?)\s*:\s*1$"));
    static VALUE: LazyLock<Regex> =
        LazyLock::new(|| regex(r"^(-?\d*\.?\d+(?:e-?\d+)?)\s*(khz|hz|ms|s|sec|db|%|st|semi|cents?|ct|bpm|x|°|deg|k)?(?![a-z])"));
    let cleaned = trim(text).replace('−', "-").replace(',', "").to_lowercase();
    if INF.is_match(&cleaned).ok()? {
        return Some(DisplayReading { value: f64::NEG_INFINITY, unit: if cleaned.contains("db") { "db" } else { "" }.into() });
    }
    if let Some(captures) = RATIO.captures(&cleaned).ok()? {
        return Some(DisplayReading { value: number::parse(captures.get(1)?.as_str()).unwrap_or(f64::NAN), unit: "ratio".into() });
    }
    let captures = VALUE.captures(&cleaned).ok()??;
    let mut value = number::parse(captures.get(1)?.as_str()).unwrap_or(f64::NAN);
    if !value.is_finite() {
        return None;
    }
    let mut unit = captures.get(2).map(|v| v.as_str()).unwrap_or("");
    match unit {
        "khz" | "k" => {
            value *= 1000.0;
            unit = "hz";
        }
        "ms" => {
            value /= 1000.0;
            unit = "s";
        }
        "sec" => unit = "s",
        "semi" => unit = "st",
        "cent" | "ct" => unit = "cents",
        "deg" => unit = "°",
        _ => {}
    }
    Some(DisplayReading { value, unit: unit.into() })
}
pub fn value_for_display(map: &DisplayMap, target: impl Into<DisplayTarget>) -> Result<f64, String> {
    let text = match target.into() {
        DisplayTarget::Number(value) => return Ok(value),
        DisplayTarget::Text(text) => text,
    };
    let text = trim(&text);
    let numeric = number::parse(text).unwrap_or(f64::NAN);
    if !text.is_empty() && numeric.is_finite() {
        return Ok(numeric);
    }
    let wanted = text.to_lowercase();
    let items = map.items.as_deref().unwrap_or_default();
    if !items.is_empty() {
        let index = items
            .iter()
            .position(|item| item.to_lowercase() == wanted)
            .or_else(|| items.iter().position(|item| item.to_lowercase().starts_with(&wanted)));
        if let Some(index) = index {
            return Ok(map.min + index as f64 * if items.len() > 1 { (map.max - map.min) / (items.len() - 1) as f64 } else { 0.0 });
        }
    }
    if let Some((value, _)) = map.grid.iter().find(|(_, label)| trim(label).to_lowercase() == wanted) {
        return Ok(*value);
    }
    let Some(goal) = parse_display(text) else {
        return Err(format!(
            "“{text}” isn't a value Kumi can place on this parameter; give a number between {} and {}{}.",
            number::to_string(map.min),
            number::to_string(map.max),
            if items.is_empty() {
                String::new()
            } else {
                format!(", or one of {}", items.iter().take(12).cloned().collect::<Vec<_>>().join(", "))
            }
        ));
    };
    let points: Vec<_> = map
        .grid
        .iter()
        .filter_map(|(value, label)| {
            parse_display(label)
                .filter(|read| read.unit == goal.unit || goal.unit.is_empty() || read.unit.is_empty())
                .map(|read| (*value, read.value))
        })
        .collect();
    if points.len() < 2 {
        return Err(format!(
            "This parameter doesn't show values in {}; give a number between {} and {}.",
            if goal.unit.is_empty() { "plain numbers" } else { &goal.unit },
            number::to_string(map.min),
            number::to_string(map.max)
        ));
    }
    let log_scale = (goal.unit == "hz" || goal.unit == "s") && points.iter().all(|(_, shown)| *shown > 0.0) && goal.value > 0.0;
    let scale = |shown: f64| if log_scale { shown.ln() } else { shown };
    let at = scale(goal.value);
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (as_, bs) = (scale(a.1), scale(b.1));
        if at < as_.min(bs) || at > as_.max(bs) {
            continue;
        }
        let span = bs - as_;
        return Ok(if span == 0.0 { a.0 } else { a.0 + (b.0 - a.0) * ((at - as_) / span) });
    }
    let (first, last) = (points[0], points[points.len() - 1]);
    Ok(if (scale(first.1) - at).abs() <= (scale(last.1) - at).abs() { first.0 } else { last.0 })
}
