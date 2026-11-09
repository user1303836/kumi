//! The map's Make options (e on the map): every choice a way of making a sound takes, one row each, kept for each way
//! while Kumi runs. Every way takes the steer (contract A1: eleven on the result, six on each source, three of those as
//! a range); Resonate also its pairing, ring and decay; Modulate its carrier, kinds, stages and retrigger; Loop its
//! tempo (the DAW's unless set), bars and every one of its choices, a beat's or a melody's as its style is. A row not
//! set is as the seed draws it. The rows become the request's `Options` through their JSON, so their keys are the
//! request's own.

use std::collections::BTreeMap;

use kumi_design::{choices, Options};
use serde_json::{Map, Number, Value};

/// What a row sets.
#[derive(Clone, Debug, PartialEq)]
pub enum Knob {
    /// A number from `low` to `high`, `step` at a time; `neutral` is where it starts from.
    Number { low: f64, high: f64, step: f64, neutral: f64 },
    /// One end (`end`: 0 the low one, 1 the high one) of a `[low, high]` range from `low` to `high`.
    End { end: usize, low: f64, high: f64, step: f64 },
    /// A whole number from `low` to `high`.
    Whole { low: i64, high: i64, neutral: i64 },
    /// One of these words (`numbers`: they're written as numbers in the request).
    Word { words: Vec<String>, numbers: bool },
    /// One of a list's words, in it or not (Modulate's kinds).
    Flag { word: String },
}

/// One row: where its value goes (`group`: "steer", "resonate", "modulate", "loop", or "" for the options themselves),
/// its key there, what the row's called, what it sets, and the loop style it's for (None: any).
#[derive(Clone, Debug)]
pub struct Row {
    pub group: &'static str,
    pub key: &'static str,
    pub label: String,
    pub knob: Knob,
    pub style: Option<&'static str>,
}

impl Row {
    /// The row's id among its way's values: its group and key, and an end's or a flag's own part.
    pub fn id(&self) -> String {
        match &self.knob {
            Knob::End { end, .. } => format!("{}.{}.{end}", self.group, self.key),
            Knob::Flag { word } => format!("{}.{}.{word}", self.group, self.key),
            _ => format!("{}.{}", self.group, self.key),
        }
    }
}

/// The ways the map makes a sound, by their ids, as the Make options name them.
pub const WAYS: [&str; 9] = ["blend", "attack", "resonate", "modulate", "texture", "loop", "smear", "stack", "grow"];

/// An enum's words as the request writes them (its serde names).
fn words<T: serde::Serialize>(all: &[T]) -> Vec<String> {
    all.iter().filter_map(|each| serde_json::to_value(each).ok()).filter_map(|value| value.as_str().map(str::to_string)).collect()
}

fn number(group: &'static str, key: &'static str, label: &str, low: f64, high: f64, step: f64) -> Row {
    let neutral = 0f64.clamp(low, high);
    Row { group, key, label: label.into(), knob: Knob::Number { low, high, step, neutral }, style: None }
}

fn range(key: &'static str, label: &str, low: f64, high: f64, step: f64) -> [Row; 2] {
    [0, 1].map(|end| Row {
        group: "steer",
        key,
        label: format!("{label} {}", if end == 0 { "from" } else { "to" }),
        knob: Knob::End { end, low, high, step },
        style: None,
    })
}

fn whole(group: &'static str, key: &'static str, label: &str, low: i64, high: i64, neutral: i64) -> Row {
    Row { group, key, label: label.into(), knob: Knob::Whole { low, high, neutral }, style: None }
}

fn word(group: &'static str, key: &'static str, label: &str, words: Vec<String>) -> Row {
    Row { group, key, label: label.into(), knob: Knob::Word { words, numbers: false }, style: None }
}

fn for_style(mut row: Row, style: &'static str) -> Row {
    row.style = Some(style);
    row
}

/// The rows a way has (by its id in `WAYS`), the steer's last.
pub fn rows(way: &str) -> Vec<Row> {
    let mut rows = vec![];
    match way {
        "resonate" => {
            rows.push(word("resonate", "pairing", "pairing", words(&choices::Pairing::ALL)));
            rows.push(word("resonate", "ring", "ring", words(&choices::Ring::ALL)));
            rows.push(number("resonate", "decay", "decay", 0., 1., 0.05));
        }
        "modulate" => {
            rows.push(word("modulate", "carrier", "carrier", words(&choices::Carrier::ALL)));
            for kind in words(&choices::Modulation::ALL) {
                rows.push(Row { group: "modulate", key: "kinds", label: format!("kind: {kind}"), knob: Knob::Flag { word: kind }, style: None });
            }
            rows.push(whole("modulate", "stages", "stages", 1, 8, 4));
            rows.push(number("modulate", "retrigger", "retrigger", 0., 1., 0.05));
        }
        "loop" => {
            rows.push(whole("", "bpm", "tempo (BPM)", 40, 240, 120));
            rows.push(whole("", "bars", "bars", 1, kumi_design::MAX_BARS as i64, 2));
            rows.push(word("loop", "style", "style", words(&choices::Style::ALL)));
            // The meter's two numbers, its beats and its unit, as two rows (`options` puts them together).
            rows.push(whole("loop", "meter", "meter beats", 1, 16, 4));
            let unit = ["2", "4", "8", "16"].map(String::from).to_vec();
            rows.push(Row { group: "loop", key: "meter-unit", label: "meter unit".into(), knob: Knob::Word { words: unit, numbers: true }, style: None });
            let grid = ["2", "3", "4", "6", "8"].map(String::from).to_vec();
            rows.push(Row { group: "loop", key: "grid", label: "steps a beat".into(), knob: Knob::Word { words: grid, numbers: true }, style: None });
            rows.push(number("loop", "swing", "swing", 0., 0.75, 0.05));
            for key in ["density", "humanize", "accent", "fills", "ratchet", "poly", "variation"] {
                rows.push(number("loop", key, key, 0., 1., 0.05));
            }
            rows.push(for_style(number("loop", "lean", "lean (start ↔ end)", -1., 1., 0.1), "beat"));
            for key in ["syncopation", "ghosts", "open", "kick", "snare", "hats", "perc"] {
                rows.push(for_style(number("loop", key, key, 0., 1., 0.05), "beat"));
            }
            let keys = choices::KEY_NAMES.map(String::from).to_vec();
            rows.push(for_style(word("loop", "key", "key", keys), "melody"));
            rows.push(for_style(word("loop", "scale", "scale", words(&choices::Scale::ALL)), "melody"));
            rows.push(for_style(word("loop", "tuning", "tuning", words(&choices::Tuning::ALL)), "melody"));
            rows.push(for_style(number("loop", "micro", "micro", 0., 1., 0.05), "melody"));
            rows.push(for_style(whole("loop", "octaves", "octaves", 1, 4, 1), "melody"));
            rows.push(for_style(whole("loop", "register", "register", -2, 2, 0), "melody"));
            rows.push(for_style(word("loop", "contour", "contour", words(&choices::Contour::ALL)), "melody"));
            for key in ["chords", "glide"] {
                rows.push(for_style(number("loop", key, key, 0., 1., 0.05), "melody"));
            }
            let mut gate = number("loop", "gate", "gate", 0.05, 1., 0.05);
            gate.knob = Knob::Number { low: 0.05, high: 1., step: 0.05, neutral: 0.5 };
            rows.push(for_style(gate, "melody"));
            rows.push(for_style(number("loop", "drone", "drone", 0., 1., 0.05), "melody"));
        }
        _ => {}
    }
    // The steer, every way's: on the result, then on each source.
    for (key, label, low) in [
        ("bright", "dark ↔ bright", -1.),
        ("body", "thin ↔ full", -1.),
        ("filter", "low-pass ↔ high-pass", -1.),
        ("resonance", "resonance", 0.),
        ("noise", "tonal ↔ noisy", -1.),
        ("punch", "soft ↔ punchy", -1.),
        ("length", "shorter ↔ longer", -1.),
        ("drive", "drive", 0.),
        ("crush", "crush", 0.),
        ("width", "mono ↔ wide", -1.),
        ("space", "space", 0.),
    ] {
        rows.push(number("steer", key, label, low, 1., 0.1));
    }
    rows.extend(range("pitch", "pitch (st)", -24., 24., 1.));
    rows.push(number("steer", "reverse", "reversed (chance)", 0., 1., 0.1));
    rows.extend(range("start", "start", 0., 0.9, 0.05));
    rows.extend(range("delay", "delay (ms)", 0., 500., 10.));
    rows.push(number("steer", "pan", "pan spread", 0., 1., 0.1));
    rows.push(number("steer", "level", "level drop", 0., 1., 0.1));
    rows
}

/// Each way's values, by row id: what's set (a number, a word, a flag); a row not here is as the seed draws it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Knobs(pub BTreeMap<String, BTreeMap<String, Value>>);

fn round(value: f64, step: f64) -> f64 {
    let steps = (value / step).round() * step;
    (steps * 1e6).round() / 1e6
}

impl Knobs {
    /// A way's value for a row.
    pub fn value(&self, way: &str, row: &Row) -> Option<&Value> {
        self.0.get(way).and_then(|values| values.get(&row.id()))
    }

    fn set(&mut self, way: &str, row: &Row, value: Option<Value>) {
        let values = self.0.entry(way.into()).or_default();
        match value {
            Some(value) => values.insert(row.id(), value),
            None => values.remove(&row.id()),
        };
    }

    /// The rows a way shows now: a loop's beat rows only for a beat (its style unset is a beat), its melody rows only
    /// for a melody.
    pub fn shown(&self, way: &str) -> Vec<Row> {
        let all = rows(way);
        let style = all
            .iter()
            .find(|row| row.group == "loop" && row.key == "style")
            .and_then(|row| self.value(way, row))
            .and_then(Value::as_str)
            .unwrap_or("beat")
            .to_string();
        all.into_iter().filter(|row| row.style.is_none_or(|only| only == style)).collect()
    }

    /// A row moved `by` steps (a word: to the next or the one before; a flag: in or out). A range's other end moves with
    /// it so its low end stays at or under its high one.
    pub fn nudge(&mut self, way: &str, row: &Row, by: i64) {
        let now = self.value(way, row).cloned();
        let value = match &row.knob {
            Knob::Number { low, high, step, neutral } => {
                let from = now.as_ref().and_then(Value::as_f64).unwrap_or(*neutral);
                Value::from(round((from + *step * by as f64).clamp(*low, *high), *step))
            }
            Knob::End { end, low, high, step } => {
                let from = now.as_ref().and_then(Value::as_f64).unwrap_or(0f64.clamp(*low, *high));
                let to = round((from + *step * by as f64).clamp(*low, *high), *step);
                let other = Row { knob: Knob::End { end: 1 - end, low: *low, high: *high, step: *step }, ..row.clone() };
                let other_value = self.value(way, &other).and_then(Value::as_f64).unwrap_or(0f64.clamp(*low, *high));
                if (*end == 0 && other_value < to) || (*end == 1 && other_value > to) {
                    self.set(way, &other, Some(Value::from(to)));
                }
                Value::from(to)
            }
            Knob::Whole { low, high, neutral } => {
                let from = now.as_ref().and_then(Value::as_i64).unwrap_or(*neutral);
                Value::from((from + by).clamp(*low, *high))
            }
            Knob::Word { words, numbers } => {
                let at = now.as_ref().and_then(|now| words.iter().position(|word| word_value(word, *numbers) == *now));
                // Not set, the first step lands on the first word (back: the last).
                let from = at.map_or(if by > 0 { -1 } else { words.len() as i64 }, |at| at as i64);
                let next = (from + by).rem_euclid(words.len() as i64) as usize;
                word_value(&words[next], *numbers)
            }
            Knob::Flag { .. } => Value::from(!now.as_ref().and_then(Value::as_bool).unwrap_or(false)),
        };
        self.set(way, row, Some(value));
    }

    /// A row as the seed draws it again.
    pub fn unset(&mut self, way: &str, row: &Row) {
        self.set(way, row, None);
    }

    /// Every row of a way as the seed draws it.
    pub fn reset(&mut self, way: &str) {
        self.0.remove(way);
    }

    /// Every row a way shows set at random (`draw`: a number in [0, 1) each time it's called), a number within the
    /// middle 60% of its range about where it starts from, so a roll stays musical.
    pub fn roll(&mut self, way: &str, draw: &mut dyn FnMut() -> f64) {
        // The loop's length (tempo, bars, meter) stays: a roll of those can run past a loop's longest.
        for row in self.shown(way).into_iter().filter(|row| !row.group.is_empty() && !row.key.starts_with("meter")) {
            let value = match &row.knob {
                Knob::Number { low, high, step, neutral } => {
                    let (from, to) = ((neutral - (neutral - low) * 0.6), (neutral + (high - neutral) * 0.6));
                    Value::from(round(from + (to - from) * draw(), *step).clamp(*low, *high))
                }
                Knob::End { end, low, high, step } => {
                    // The low end first, then the high one from it.
                    let low_now = if *end == 0 { None } else { self.value(way, &row_end(&row, 0)).and_then(Value::as_f64) };
                    let from = low_now.unwrap_or(0f64.clamp(*low, *high) - (0f64.clamp(*low, *high) - low) * 0.6);
                    let to = high * 0.6;
                    Value::from(round((from + (to - from).max(0.) * draw()).clamp(*low, *high), *step))
                }
                Knob::Whole { low, high, .. } => Value::from(low + ((high - low + 1) as f64 * draw()).floor().min((high - low) as f64) as i64),
                Knob::Word { words, numbers } => word_value(&words[((words.len() as f64 * draw()) as usize).min(words.len() - 1)], *numbers),
                Knob::Flag { .. } => Value::from(draw() < 0.5),
            };
            self.set(way, &row, Some(value));
        }
        // At least one kind of modulation stays in.
        let kinds: Vec<Row> = self.shown(way).into_iter().filter(|row| matches!(row.knob, Knob::Flag { .. })).collect();
        if !kinds.is_empty() && kinds.iter().all(|row| self.value(way, row) == Some(&Value::from(false))) {
            self.set(way, &kinds[0], Some(Value::from(true)));
        }
    }

    /// How a row's value reads in the editor: "as drawn" where it's not set.
    pub fn written(&self, way: &str, row: &Row) -> String {
        match (self.value(way, row), &row.knob) {
            (None, _) => "drawn".into(),
            (Some(Value::Bool(on)), _) => if *on { "in" } else { "out" }.into(),
            (Some(Value::String(word)), _) => word.clone(),
            (Some(value), _) => value.to_string(),
        }
    }

    /// The way's options for a request, through their JSON: the rows set, a range's unset end at 0, the kinds of
    /// modulation in, and `tempo` (the DAW's) for a loop whose tempo isn't set. Why not, where the engine refuses them.
    pub fn options(&self, way: &str, tempo: Option<f64>) -> Result<Options, String> {
        let mut top = Map::new();
        let mut groups: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
        let mut kinds: Option<Vec<Value>> = None;
        let mut meter: Option<[Value; 2]> = None;
        for row in self.shown(way) {
            let value = self.value(way, &row);
            match &row.knob {
                Knob::End { end, low, high, .. } => {
                    let Some(value) = value else { continue };
                    let group = groups.entry(row.group).or_default();
                    let neutral = 0f64.clamp(*low, *high);
                    let pair = group.entry(row.key.to_string()).or_insert_with(|| Value::Array(vec![neutral.into(), neutral.into()]));
                    pair[*end] = value.clone();
                }
                _ if row.key == "meter" || row.key == "meter-unit" => {
                    let Some(value) = value else { continue };
                    let pair = meter.get_or_insert_with(|| [Value::from(4), Value::from(4)]);
                    pair[usize::from(row.key == "meter-unit")] = value.clone();
                }
                Knob::Flag { .. } => {
                    // Touching any kind lists them all: those not taken out stay in, in their order.
                    if value.is_some() && kinds.is_none() {
                        let shown = self.shown(way);
                        kinds = Some(
                            shown
                                .iter()
                                .filter_map(|row| match &row.knob {
                                    Knob::Flag { word } => self.value(way, row).and_then(Value::as_bool).unwrap_or(true).then(|| Value::from(word.clone())),
                                    _ => None,
                                })
                                .collect(),
                        );
                    }
                }
                _ => {
                    let Some(value) = value else { continue };
                    if row.group.is_empty() {
                        top.insert(row.key.into(), value.clone());
                    } else {
                        groups.entry(row.group).or_default().insert(row.key.into(), value.clone());
                    }
                }
            }
        }
        if let Some(meter) = meter {
            groups.entry("loop").or_default().insert("meter".into(), Value::Array(meter.to_vec()));
        }
        if let Some(kinds) = kinds {
            groups.entry("modulate").or_default().insert("kinds".into(), Value::Array(kinds));
        }
        if way == "loop" && !top.contains_key("bpm") {
            if let Some(tempo) = tempo.filter(|tempo| (40.0..=240.).contains(tempo)).and_then(Number::from_f64) {
                top.insert("bpm".into(), Value::Number(tempo));
            }
        }
        for (group, values) in groups {
            top.insert(group.into(), Value::Object(values));
        }
        serde_json::from_value(Value::Object(top)).map_err(|error| format!("Kumi couldn't take those options: {error}"))
    }
}

/// A word as the request writes it.
fn word_value(word: &str, numbers: bool) -> Value {
    if numbers {
        word.parse::<u64>().map(Value::from).unwrap_or_else(|_| Value::from(word))
    } else {
        Value::from(word)
    }
}

/// A range's row for its other end.
fn row_end(row: &Row, end: usize) -> Row {
    let Knob::End { low, high, step, .. } = row.knob else { return row.clone() };
    Row { knob: Knob::End { end, low, high, step }, ..row.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(rows: &'a [Row], label: &str) -> &'a Row {
        rows.iter().find(|row| row.label == label).unwrap_or_else(|| panic!("no {label}"))
    }

    #[test]
    fn every_steer_control_is_a_row_of_every_way_and_its_values_reach_the_options() {
        for way in WAYS {
            let steer: Vec<Row> = rows(way).into_iter().filter(|row| row.group == "steer").collect();
            let keys: std::collections::BTreeSet<&str> = steer.iter().map(|row| row.key).collect();
            assert_eq!(keys.len(), 17, "{way}: {keys:?}");
        }
        let mut knobs = Knobs::default();
        let all = rows("blend");
        knobs.nudge("blend", row(&all, "dark ↔ bright"), 3);
        knobs.nudge("blend", row(&all, "soft ↔ punchy"), -2);
        // A range: its high end up, then its low end past it, which pushes the high end along.
        knobs.nudge("blend", row(&all, "pitch (st) to"), 3);
        knobs.nudge("blend", row(&all, "pitch (st) from"), -5);
        knobs.nudge("blend", row(&all, "pitch (st) from"), 9);
        knobs.nudge("blend", row(&all, "delay (ms) to"), 100);
        let options = knobs.options("blend", None).expect("options");
        let steer = options.steer.expect("a steer");
        assert_eq!((steer.bright, steer.punch), (Some(0.3), Some(-0.2)));
        assert_eq!(steer.pitch, Some([4., 4.]));
        assert_eq!(steer.delay, Some([0., 500.]), "clamped to its range");
        assert_eq!(steer.body, None, "not set: as drawn");
        // Each way keeps its own; reset lets them all go.
        assert!(knobs.options("smear", None).expect("options").steer.is_none());
        knobs.reset("blend");
        assert_eq!(knobs.options("blend", None).expect("options"), Options::default());
    }

    #[test]
    fn resonate_modulate_and_loop_choices_reach_the_options() {
        let mut knobs = Knobs::default();
        let resonate = rows("resonate");
        knobs.nudge("resonate", row(&resonate, "pairing"), 3);
        knobs.nudge("resonate", row(&resonate, "decay"), 14);
        let options = knobs.options("resonate", None).unwrap();
        let resonate = options.resonate.unwrap();
        assert_eq!(serde_json::to_value(resonate.pairing).unwrap(), "interleave");
        assert_eq!(resonate.decay, Some(0.7));
        let modulate = rows("modulate");
        knobs.nudge("modulate", row(&modulate, "kind: vocode"), 1);
        knobs.nudge("modulate", row(&modulate, "kind: vocode"), 1);
        knobs.nudge("modulate", row(&modulate, "kind: fm"), 1);
        let kinds = knobs.options("modulate", None).unwrap().modulate.unwrap().kinds.unwrap();
        assert_eq!(serde_json::to_value(&kinds).unwrap(), serde_json::json!(["phase", "ring", "am", "fm", "filter", "gain"]));
        // A loop: the DAW's tempo unless set; a melody's rows show only for a melody, and reach its options.
        assert_eq!(knobs.options("loop", Some(97.)).unwrap().bpm, Some(97.));
        assert!(knobs.shown("loop").iter().all(|row| row.key != "scale"));
        let looped = rows("loop");
        knobs.nudge("loop", row(&looped, "style"), 2);
        let shown = knobs.shown("loop");
        assert!(shown.iter().any(|row| row.key == "scale") && shown.iter().all(|row| row.key != "ghosts"));
        knobs.nudge("loop", row(&shown, "scale"), 2);
        knobs.nudge("loop", row(&shown, "meter beats"), 3);
        knobs.nudge("loop", row(&shown, "meter unit"), 3);
        knobs.nudge("loop", row(&shown, "tempo (BPM)"), 4);
        let options = knobs.options("loop", Some(97.)).unwrap();
        assert_eq!(options.bpm, Some(124.), "set, it's the loop's own");
        let looped = options.r#loop.unwrap();
        assert_eq!(serde_json::to_value(looped.style).unwrap(), "melody");
        assert_eq!(serde_json::to_value(looped.scale).unwrap(), "minor");
        assert_eq!(looped.meter, Some([7, 8]));
    }

    #[test]
    fn a_roll_sets_every_row_shown_within_its_range() {
        let mut knobs = Knobs::default();
        let mut seed = 0.37f64;
        let mut draw = || {
            seed = (seed * 9301. + 0.49297).fract();
            seed
        };
        for way in WAYS {
            knobs.roll(way, &mut draw);
            for row in knobs.shown(way).into_iter().filter(|row| !row.group.is_empty() && !row.key.starts_with("meter")) {
                assert!(knobs.value(way, &row).is_some(), "{way}: {}", row.label);
            }
            knobs.options(way, None).unwrap_or_else(|why| panic!("{way}: {why}"));
        }
    }
}
