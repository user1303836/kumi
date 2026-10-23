//! What HISTORY draws of a change in a DAW whose bridge is Kumi's own, one picture per family of kinds, from what the
//! bridge answered: what was there (the preview's `prior` and `display`) and what's there now (the apply's `applied` and
//! `display`), in the DAW's own words. A tracker's lines, an envelope's or automation's points, the parameters that
//! moved, a route between tracks, a keyzone, items on the timeline, and the settings that moved.
use kumi_types::contracts::{ChangePicture, ClipNote, DevicePlacement};
use kumi_types::daw::DawProfile;
use kumi_types::picture::{PictureItem, PictureLine, PictureRoute, PictureValue, PictureZone, RouteChange};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// What a change answered: its preview (`prior`, `display`, `target`) and its apply (`applied`, `display`).
pub struct Answers<'a> {
    pub preview: &'a Value,
    pub applied: &'a Value,
}

impl Answers<'_> {
    fn prior(&self) -> &Value {
        self.preview.get("prior").unwrap_or(&Value::Null)
    }
    fn after(&self) -> &Value {
        self.applied.get("applied").unwrap_or(&Value::Null)
    }
    /// What was there as shown: the apply's `display.before` when it says, else the preview's.
    fn shown_before(&self) -> &Value {
        self.applied
            .pointer("/display/before")
            .filter(|before| before.is_object())
            .or_else(|| self.preview.pointer("/display/before"))
            .unwrap_or(&Value::Null)
    }
    fn shown(&self, key: &str) -> &Value {
        self.applied.pointer(&format!("/display/{key}")).unwrap_or(&Value::Null)
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A value as the DAW wrote it ("-6.021 dB", "-INF dB"), with the true minus before its numbers, as Kumi's titles write them.
fn minus_text(text: String) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 2);
    for (k, c) in chars.iter().enumerate() {
        let starts = k == 0 || chars[k - 1].is_whitespace() || chars[k - 1] == '(';
        let number = chars.get(k + 1).is_some_and(|next| next.is_ascii_digit() || *next == '.' || next.eq_ignore_ascii_case(&'i'));
        out.push(if *c == '-' && starts && number { '−' } else { *c });
    }
    out
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// A value as the bridge drew it (`{name, from?, to?, min?, max?, fromText?, toText?}`).
fn drawn_value(value: &Value) -> Option<PictureValue> {
    Some(PictureValue {
        name: text(value, "name")?,
        from: number(value, "from"),
        to: number(value, "to"),
        min: number(value, "min"),
        max: number(value, "max"),
        from_text: text(value, "fromText").map(minus_text),
        to_text: text(value, "toText").map(minus_text),
        asked: None,
    })
}

// ----- a tracker's lines -----

/// Renoise's empty fields, as its editor draws them.
const EMPTY_FX: &str = "0000";

/// One cell of a line as the DAW writes it: a note column's note, instrument, volume, panning and delay (and its sample
/// effect, `fx`, when the picture shows them), or an effect column's number and amount ("0A10"); empty fields as the
/// DAW draws them.
fn cell_text(cell: &Value, fx: bool) -> String {
    let field = |key: &str, empty: &str| text(cell, key).filter(|s| !s.is_empty()).unwrap_or_else(|| empty.to_owned());
    if cell.get("kind").and_then(Value::as_str) == Some("effect") {
        let (number, amount) = (field("number", "00"), field("amount", "00"));
        return if number == "00" && amount == "00" { "....".into() } else { format!("{number}{amount}") };
    }
    let mut fields =
        vec![field("note", "---"), field("instrument", ".."), field("volume", ".."), field("panning", ".."), field("delay", "..")];
    if fx {
        let effect = field("fx", EMPTY_FX);
        fields.push(if effect == EMPTY_FX { "....".into() } else { effect });
    }
    fields.join(" ")
}

/// The lines a change wrote, before and after: `cells` in `prior` and `applied` are the same cells (each a line's note or
/// effect column, its line a place from 1), in the same order. Each line shows the columns the change wrote, notes first,
/// by Renoise's number (from 0).
pub fn lines(answers: &Answers) -> Option<ChangePicture> {
    let (before, after) = (list(answers.prior(), "cells"), list(answers.after(), "cells"));
    if before.len() != after.len() || before.is_empty() {
        return None;
    }
    let fx = before.iter().chain(after).any(|cell| text(cell, "fx").is_some_and(|fx| !fx.is_empty() && fx != EMPTY_FX));
    // By line, then each column in its place: notes before effects, by column.
    let mut by_line: BTreeMap<u32, BTreeMap<(bool, u64), (String, String)>> = BTreeMap::new();
    for (was, now) in before.iter().zip(after) {
        let line = number(was, "line").filter(|n| *n >= 1.)? as u32 - 1;
        let column = (was.get("kind").and_then(Value::as_str) == Some("effect"), number(was, "col").unwrap_or(1.) as u64);
        by_line.entry(line).or_default().insert(column, (cell_text(was, fx), cell_text(now, fx)));
    }
    let lines: Vec<PictureLine> = by_line
        .into_iter()
        .map(|(line, cells)| {
            let (before, after): (Vec<String>, Vec<String>) = cells.into_values().unzip();
            PictureLine { line, before: before.join(" "), after: after.join(" ") }
        })
        .filter(|line| line.before != line.after)
        .collect();
    (!lines.is_empty()).then_some(ChangePicture::Lines { lines, more: 0 })
}

// ----- an envelope's or automation's points -----

/// A lane's points before and after: as the bridge drew them (`display.curve`: REAPER's envelopes, in their units), or a
/// pattern's automation (Renoise: `points` of `{time, value}`, time in lines, value 0 to 1 of the parameter's range;
/// none when it was deleted; the parameter named by `display.parameter`).
pub fn curve(answers: &Answers) -> Option<ChangePicture> {
    let pairs = |points: &[Value]| -> Vec<[f64; 2]> {
        points
            .iter()
            .filter_map(|point| match point {
                Value::Array(pair) => Some([pair.first()?.as_f64()?, pair.get(1)?.as_f64()?]),
                point => Some([number(point, "time")?, number(point, "value")?]),
            })
            .collect()
    };
    let drawn = answers.shown("curve");
    if drawn.is_object() {
        // (project times: drawn from the first point of either side, so a lane deep in the song isn't squeezed to its end)
        let (mut before, mut after) = (pairs(list(drawn, "before")), pairs(list(drawn, "after")));
        let first = before.iter().chain(&after).map(|point| point[0]).fold(f64::INFINITY, f64::min);
        for point in before.iter_mut().chain(after.iter_mut()) {
            point[0] -= first;
        }
        return Some(ChangePicture::Automation {
            parameter: text(drawn, "parameter").unwrap_or_default(),
            min: number(drawn, "min")?,
            max: number(drawn, "max")?,
            before,
            after,
            words: text(drawn, "words").map(minus_text),
        });
    }
    let side = |state: &Value| if state.get("exists") == Some(&Value::Bool(false)) { vec![] } else { pairs(list(state, "points")) };
    let (before, after) = (side(answers.prior()), side(answers.after()));
    let words = lane_words(if after.is_empty() { &before } else { &after });
    (answers.after().get("points").is_some() || answers.after().get("exists").is_some()).then_some(ChangePicture::Automation {
        parameter: answers.shown("parameter").as_str().unwrap_or_default().to_owned(),
        min: 0.,
        max: 1.,
        before,
        after,
        words,
    })
}

/// What a pattern's automation moves between and where, in words: "10% to 100% over lines 00–24" (its values are of the
/// parameter's range; its times are lines from 1, which Renoise numbers from 00).
fn lane_words(points: &[[f64; 2]]) -> Option<String> {
    let (first, last) = (points.first()?, points.last()?);
    let (low, high) = points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), point| (low.min(point[1]), high.max(point[1])));
    let percent = |value: f64| format!("{}%", plain_number((value * 100.).round()));
    let range = if percent(low) == percent(high) { percent(low) } else { format!("{} to {}", percent(low), percent(high)) };
    let line = |time: f64| format!("{:02}", (time - 1.).max(0.).floor() as i64);
    Some(if line(first[0]) == line(last[0]) {
        format!("{range} at line {}", line(first[0]))
    } else {
        format!("{range} over lines {}–{}", line(first[0]), line(last[0]))
    })
}

// ----- parameters -----

/// "Cutoff → 1.20 kHz": a parameter and what it shows now, as a bridge's display lines say it.
fn moved_line(line: &str) -> Option<(String, String)> {
    let (name, shown) = line.rsplit_once(" → ")?;
    Some((name.trim().to_owned(), shown.trim().to_owned()))
}

/// The parameters that moved, each in the DAW's words: as the bridge drew them (`display.values`: REAPER's); one
/// parameter the display names (`display.parameter`, its range `min`/`max`, and `before`/`after` as `{value,
/// valueString}`: Renoise's device-param and its kin); else what the preview showed of each (`display.before`, by name)
/// and the apply's display lines ("Name → what it shows").
pub fn params(answers: &Answers) -> Option<ChangePicture> {
    let drawn: Vec<PictureValue> =
        list(answers.applied.get("display").unwrap_or(&Value::Null), "values").iter().filter_map(drawn_value).collect();
    if !drawn.is_empty() {
        return Some(ChangePicture::Values { values: drawn });
    }
    if let Some(name) = answers.shown("parameter").as_str() {
        let (before, after) = (answers.shown_before(), answers.shown("after"));
        let value = PictureValue {
            name: name.to_owned(),
            from: number(before, "value"),
            to: number(after, "value"),
            min: answers.shown("min").as_f64(),
            max: answers.shown("max").as_f64(),
            from_text: text(before, "valueString").map(minus_text),
            to_text: text(after, "valueString").map(minus_text),
            asked: None,
        };
        return Some(ChangePicture::Values { values: vec![value] });
    }
    let before = answers.shown_before();
    let values: Vec<PictureValue> = list(answers.applied.get("display").unwrap_or(&Value::Null), "lines")
        .iter()
        .filter_map(Value::as_str)
        .filter_map(moved_line)
        .map(|(name, shown)| PictureValue {
            from_text: before
                .get(&name)
                .and_then(|was| was.as_str().map(|t| minus_text(t.to_owned())).or_else(|| was.as_f64().map(plain_number))),
            to_text: Some(minus_text(shown)),
            name,
            ..Default::default()
        })
        .collect();
    (!values.is_empty()).then_some(ChangePicture::Values { values })
}

// ----- a route -----

/// A route as the bridge drew it (`display.route`: its ends, whether it was made, removed or changed, and the values that
/// moved on it).
pub fn route(answers: &Answers) -> Option<ChangePicture> {
    let drawn = answers.shown("route");
    let change = match drawn.get("change").and_then(Value::as_str)? {
        "made" => RouteChange::Made,
        "removed" => RouteChange::Removed,
        _ => RouteChange::Changed,
    };
    let route = PictureRoute {
        from: text(drawn, "from").unwrap_or_default(),
        to: text(drawn, "to").unwrap_or_default(),
        change,
        values: list(drawn, "values").iter().filter_map(drawn_value).collect(),
    };
    Some(ChangePicture::Routes { routes: vec![route] })
}

// ----- a keyzone -----

/// A keyzone before and after (Renoise's): its notes, velocities and base note, as the preview and the apply showed the
/// settings the change named. What it didn't name it didn't change, and isn't claimed: velocities it didn't name are
/// left unknown (the zone is drawn by its keys alone), and a change that named no keys draws its values alone.
pub fn zone(answers: &Answers) -> Option<ChangePicture> {
    let range = |state: &Value, key: &str| -> Option<[u8; 2]> {
        let ends: Vec<u8> = list(state, key).iter().filter_map(Value::as_f64).map(|n| n.clamp(0., 127.) as u8).collect();
        match ends.as_slice() {
            [low, high] => Some([*low.min(high), *low.max(high)]),
            _ => None,
        }
    };
    let zone = |state: &Value| PictureZone {
        name: String::new(),
        notes: range(state, "noteRange").unwrap_or([0, 119]),
        velocities: range(state, "velocityRange"),
        base: number(state, "baseNote").map(|n| n.clamp(0., 127.) as u8),
    };
    let (before, after) = (answers.shown_before(), answers.shown("after"));
    // A whole instrument laid out (Renoise's Drumkit layout): every zone before and after, by its sample's place.
    if let (Some(was), Some(now)) = (before.get("zones").and_then(Value::as_array), after.get("zones").and_then(Value::as_array)) {
        let (was, now): (Vec<PictureZone>, Vec<PictureZone>) = (was.iter().map(zone).collect(), now.iter().map(zone).collect());
        let moved = was.iter().zip(&now).filter(|(a, b)| a != b).count();
        let said = format!("{moved} of {} moved", now.len());
        let values = vec![PictureValue { name: "Zones".into(), to_text: Some(said), ..Default::default() }];
        return Some(ChangePicture::Zones { before: was, after: now, values });
    }
    if !["noteRange", "velocityRange", "baseNote"].iter().any(|key| after.get(*key).is_some()) {
        return None;
    }
    let (was, now) = (zone(before), zone(after));
    // Keys in Renoise's names, as FOCUS's keyzone map says them.
    let name = |n: u8| DawProfile::RENOISE.note_name(n);
    let span = |[low, high]: [u8; 2], words: &dyn Fn(u8) -> String| {
        if low == high {
            words(low)
        } else {
            format!("{}–{}", words(low), words(high))
        }
    };
    let mut values = vec![];
    let mut moved = |label: &str, from: String, to: String| {
        if from != to {
            values.push(PictureValue { name: label.into(), from_text: Some(from), to_text: Some(to), ..Default::default() });
        }
    };
    let keys = range(after, "noteRange").is_some();
    if keys {
        moved("Keys", span(was.notes, &name), span(now.notes, &name));
    }
    if let (Some(from), Some(to)) = (was.velocities, now.velocities) {
        moved("Velocities", span(from, &|n| n.to_string()), span(to, &|n| n.to_string()));
    }
    if let (Some(from), Some(to)) = (was.base, now.base) {
        moved("Base note", name(from), name(to));
    }
    values.extend(
        settings_values(before, after)
            .into_iter()
            .filter(|value| !["Note range", "Velocity range", "Base note"].contains(&value.name.as_str())),
    );
    // (no keys named: where the zone sits isn't known, so only what moved is said)
    if !keys {
        return (!values.is_empty()).then_some(ChangePicture::Values { values });
    }
    Some(ChangePicture::Zones { before: vec![was], after: vec![now], values })
}

// ----- items -----

/// Items before and after as the bridge drew them (`display.items`: REAPER's, in seconds), with how the first one moved:
/// where it starts (as REAPER's ruler says it), how long it is, and its fades.
pub fn items(answers: &Answers) -> Option<ChangePicture> {
    let drawn = answers.shown("items");
    let item = |value: &Value| -> Option<PictureItem> {
        Some(PictureItem {
            name: text(value, "name").unwrap_or_default(),
            start: number(value, "start")?,
            length: number(value, "length")?,
            fade_in: number(value, "fadeIn").unwrap_or(0.),
            fade_out: number(value, "fadeOut").unwrap_or(0.),
            muted: value.get("muted") == Some(&Value::Bool(true)),
        })
    };
    let (before, after) = (list(drawn, "before"), list(drawn, "after"));
    if before.is_empty() && after.is_empty() {
        return None;
    }
    let mut values = vec![];
    if let ([was], [now]) = (before, after) {
        let seconds = |n: f64| format!("{} s", plain_number(n));
        let mut moved = |name: &str, words: &dyn Fn(&Value) -> Option<String>| {
            let (from, to) = (words(was), words(now));
            if from != to {
                values.push(PictureValue { name: name.into(), from_text: from, to_text: to, ..Default::default() });
            }
        };
        moved("Start", &|item| text(item, "at"));
        for (name, key) in [("Length", "length"), ("Fade in", "fadeIn"), ("Fade out", "fadeOut")] {
            moved(name, &|item| number(item, key).map(seconds));
        }
    }
    Some(ChangePicture::Items { before: before.iter().filter_map(item).collect(), after: after.iter().filter_map(item).collect(), values })
}

// ----- a take's MIDI -----

/// The notes or MIDI events a change touched, before and after, as its preview drew them (`display.before` and `after`,
/// in quarter notes from the item's start: REAPER's): notes as a clip's, from the bar the first of them is in, so notes
/// deep in a take aren't squeezed to its end; events as a controller lane's points, when they're all of one lane.
pub fn midi(answers: &Answers) -> Option<ChangePicture> {
    let display = answers.preview.get("display").unwrap_or(&Value::Null);
    let (before, after) = (list(display, "before"), list(display, "after"));
    let rows = || before.iter().chain(after);
    rows().next()?;
    if rows().all(|row| row.get("pitch").is_some() && row.get("length").is_some()) {
        let first = rows().filter_map(|row| number(row, "start")).fold(f64::INFINITY, f64::min);
        let first = (first / 4.).floor() * 4.;
        let notes = |rows: &[Value]| -> Vec<ClipNote> {
            rows.iter()
                .filter_map(|row| {
                    Some(ClipNote {
                        pitch: number(row, "pitch")?,
                        start: number(row, "start")? - first,
                        duration: number(row, "length")?,
                        velocity: number(row, "velocity").unwrap_or(100.),
                    })
                })
                .collect()
        };
        let (before, after) = (notes(before), notes(after));
        let end = before.iter().chain(&after).map(|note| note.start + note.duration).fold(0., f64::max);
        // (the preview draws so many notes; the count says how many it touched)
        let partial = number(display, "count").is_some_and(|count| count as usize > before.len().max(after.len()));
        return Some(ChangePicture::Notes { length: (end / 4.).ceil().max(1.) * 4., before, after, partial });
    }
    // One controller lane: its name, and the span of its values as REAPER's MIDI editor draws it.
    let lane = |row: &Value| (text(row, "type"), number(row, "controller"), number(row, "channel"));
    let one = lane(rows().next()?);
    if rows().any(|row| lane(row) != one) {
        return None;
    }
    let (kind, controller, _) = one;
    let (parameter, min, max) = match kind.as_deref()? {
        "cc" => (format!("CC {}", controller? as i64), 0., 127.),
        "pitchbend" => ("Pitch".into(), -8192., 8191.),
        "pressure" => ("Channel pressure".into(), 0., 127.),
        _ => return None,
    };
    let points =
        |rows: &[Value]| -> Vec<[f64; 2]> { rows.iter().filter_map(|row| Some([number(row, "start")?, number(row, "value")?])).collect() };
    let (mut before, mut after) = (points(before), points(after));
    // (drawn from the first event of either side, as a lane deep in the take)
    let first = before.iter().chain(&after).map(|point| point[0]).fold(f64::INFINITY, f64::min);
    for point in before.iter_mut().chain(after.iter_mut()) {
        point[0] -= first;
    }
    Some(ChangePicture::Automation { parameter, min, max, before, after, words: None })
}

// ----- a track's devices -----

/// A track's devices after a chain was loaded onto it (Renoise's device-chain-load: `applied.devices`, by name), drawn as
/// HISTORY draws where a device went.
pub fn chain(answers: &Answers) -> Option<DevicePlacement> {
    let devices: Vec<String> = list(answers.after(), "devices").iter().filter_map(Value::as_str).map(str::to_owned).collect();
    (!devices.is_empty()).then(|| DevicePlacement { devices: Some(devices), ..Default::default() })
}

// ----- settings -----

/// A number as a change says it: at most three places, without trailing zeros, with a true minus.
fn plain_number(n: f64) -> String {
    let text = kumi_common::js::number::to_string((n * 1000.).round() / 1000.);
    match text.strip_prefix('-') {
        Some(rest) => format!("−{rest}"),
        None => text,
    }
}

/// A setting's name from its key: "fineTune" is "Fine tune", "volumeDb" is "Volume".
fn setting_name(key: &str) -> String {
    let named = match key {
        "nna" => Some("New note action"),
        "lpb" => Some("Lines per beat"),
        "tpl" => Some("Ticks per line"),
        "bpm" => Some("BPM"),
        _ => None,
    };
    if let Some(named) = named {
        return named.into();
    }
    let key = key.strip_suffix("Db").unwrap_or(key);
    let mut words = String::new();
    for (index, c) in key.chars().enumerate() {
        if index == 0 {
            words.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            words.push(' ');
            words.extend(c.to_lowercase());
        } else {
            words.push(c);
        }
    }
    words
}

/// Whether a text is one of the DAW's refs ("3:modset:1.2", "r:track:{…}"): a ref is no word for the producer.
fn looks_like_ref(text: &str) -> bool {
    let parts: Vec<&str> = text.splitn(3, ':').collect();
    parts.len() == 3
        && !parts[0].is_empty()
        && !parts[0].contains(' ')
        && parts[1].chars().all(|c| c.is_ascii_lowercase())
        && !parts[1].is_empty()
}

/// A setting's value in words: dB with a true minus, on or off, a range "36–48", a name.
fn setting_words(key: &str, value: &Value) -> Option<String> {
    Some(match value {
        Value::String(text) if looks_like_ref(text) => return None,
        Value::String(text) => text.clone(),
        Value::Bool(on) => (if *on { "on" } else { "off" }).into(),
        Value::Number(n) if key.ends_with("Db") => format!("{} dB", plain_number(n.as_f64()?)),
        Value::Number(n) => plain_number(n.as_f64()?),
        Value::Array(ends) if ends.len() == 2 => format!("{}–{}", setting_words(key, &ends[0])?, setting_words(key, &ends[1])?),
        _ => return None,
    })
}

/// Each setting that moved, from what the preview showed (`before`) and the apply shows (`after`), by key.
fn settings_values(before: &Value, after: &Value) -> Vec<PictureValue> {
    let empty = Map::new();
    let after = after.as_object().unwrap_or(&empty);
    after
        .iter()
        .filter(|(key, now)| before.get(key.as_str()) != Some(*now))
        .filter_map(|(key, now)| {
            let to_text = setting_words(key, now)?;
            let (from, to) = (before.get(key.as_str()).and_then(Value::as_f64), now.as_f64());
            let range = match key.as_str() {
                "volumeDb" => Some((-60., 12.)),
                "pan" | "panning" => Some((-1., 1.)),
                _ => None,
            };
            Some(PictureValue {
                name: setting_name(key),
                from: from.filter(|_| range.is_some()).map(|n| n.max(-60.)),
                to: to.filter(|_| range.is_some()).map(|n| n.max(-60.)),
                min: range.map(|(min, _)| min),
                max: range.map(|(_, max)| max),
                from_text: before.get(key.as_str()).and_then(|was| setting_words(key, was)),
                to_text: Some(to_text),
                asked: None,
            })
        })
        .collect()
}

/// The settings that moved, by name, as the preview showed them before and the apply shows them now.
pub fn settings(answers: &Answers) -> Option<ChangePicture> {
    let values = settings_values(answers.shown_before(), answers.shown("after"));
    (!values.is_empty()).then_some(ChangePicture::Values { values })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A preview's answer and an apply's.
    fn answers(preview: Value, applied: Value) -> (Value, Value) {
        (preview, applied)
    }

    fn zone_picture(preview: &Value, applied: &Value) -> Option<ChangePicture> {
        zone(&Answers { preview, applied })
    }

    #[test]
    fn a_trackers_cells_are_drawn_line_by_line_in_its_own_notation() {
        let note = |line: u32, note: &str, instrument: &str, volume: &str| json!({"line": line, "kind": "note", "col": 1, "note": note, "instrument": instrument, "volume": volume, "panning": "..", "delay": "..", "fx": "0000"});
        let effect =
            |line: u32, number: &str, amount: &str| json!({"line": line, "kind": "effect", "col": 1, "number": number, "amount": amount});
        let (preview, applied) = answers(
            json!({"prior": {"cells": [note(1, "---", "..", ".."), effect(1, "00", "00"), note(5, "C-4", "01", ".."), note(9, "E-4", "01", "40")]}}),
            json!({"applied": {"cells": [note(1, "C-4", "00", "80"), effect(1, "0A", "10"), note(5, "C-4", "01", ".."), note(9, "OFF", "..", "..")]}}),
        );
        let Some(ChangePicture::Lines { lines, more }) = lines(&Answers { preview: &preview, applied: &applied }) else { panic!() };
        assert_eq!(more, 0);
        assert_eq!(
            lines,
            [
                PictureLine { line: 0, before: "--- .. .. .. .. ....".into(), after: "C-4 00 80 .. .. 0A10".into() },
                PictureLine { line: 8, before: "E-4 01 40 .. ..".into(), after: "OFF .. .. .. ..".into() },
            ],
            "line 5 was written as it was, so it isn't drawn"
        );
        // A sample effect shows as its own column, on every line.
        let fx = json!({"line": 2, "kind": "note", "col": 1, "note": "C-4", "instrument": "00", "volume": "..", "panning": "..", "delay": "..", "fx": "0S20"});
        let mut was = fx.clone();
        was["fx"] = json!("0000");
        let (preview, applied) = answers(json!({"prior": {"cells": [was]}}), json!({"applied": {"cells": [fx]}}));
        let Some(ChangePicture::Lines { lines: drawn, .. }) = super::lines(&Answers { preview: &preview, applied: &applied }) else {
            panic!()
        };
        assert_eq!((drawn[0].before.as_str(), drawn[0].after.as_str()), ("C-4 00 .. .. .. ....", "C-4 00 .. .. .. 0S20"));
    }

    #[test]
    fn a_lane_is_drawn_from_the_bridges_curve_or_a_patterns_automation() {
        let (preview, applied) = answers(
            json!({"prior": {"points": [[0.0, 0.5, 0, 0.0, false]]}}),
            json!({"display": {"curve": {"parameter": "Volume", "min": -60.0, "max": 12.0, "before": [[60.0, -6.0]], "after": [[60.0, -6.0], [62.0, 0.0]],
                "words": "-6 to 0 dB over bars 31–32"}}}),
        );
        assert_eq!(
            curve(&Answers { preview: &preview, applied: &applied }),
            Some(ChangePicture::Automation {
                parameter: "Volume".into(),
                min: -60.,
                max: 12.,
                before: vec![[0., -6.]],
                after: vec![[0., -6.], [2., 0.]],
                words: Some("−6 to 0 dB over bars 31–32".into()),
            })
        );
        let (preview, applied) = answers(
            json!({"prior": {"exists": false}}),
            json!({"applied": {"exists": true, "playmode": "linear", "points": [{"time": 1, "value": 0.0}, {"time": 33, "value": 1.0}]}}),
        );
        let Some(ChangePicture::Automation { min, max, before, after, words, .. }) =
            curve(&Answers { preview: &preview, applied: &applied })
        else {
            panic!()
        };
        assert_eq!((min, max, before.len(), after), (0., 1., 0, vec![[1., 0.], [33., 1.]]));
        // (review U20) In words too: the span of its values and where, by Renoise's line numbers.
        assert_eq!(words.as_deref(), Some("0% to 100% over lines 00–32"));
        // The parameter, as the display names it.
        let named = json!({"display": {"parameter": "Gain"}, "applied": {"exists": true, "points": [{"time": 1, "value": 0.5}]}});
        let Some(ChangePicture::Automation { parameter, .. }) = curve(&Answers { preview: &preview, applied: &named }) else { panic!() };
        assert_eq!(parameter, "Gain");
    }

    #[test]
    fn parameters_say_what_each_showed_and_shows() {
        // Renoise's: what the preview showed, by name, and the apply's lines.
        let (preview, applied) = answers(
            json!({"display": {"before": {"Gain": "0.000 dB", "Q": "1.000"}}}),
            json!({"display": {"lines": ["Gain → -6.000 dB", "Q → 2.000", "Dest. → “Gain” of “Gainer” on “Pad”"]}}),
        );
        let Some(ChangePicture::Values { values }) = params(&Answers { preview: &preview, applied: &applied }) else { panic!() };
        let said: Vec<(&str, Option<&str>, Option<&str>)> =
            values.iter().map(|v| (v.name.as_str(), v.from_text.as_deref(), v.to_text.as_deref())).collect();
        assert_eq!(
            said,
            [
                ("Gain", Some("0.000 dB"), Some("−6.000 dB")),
                ("Q", Some("1.000"), Some("2.000")),
                ("Dest.", None, Some("“Gain” of “Gainer” on “Pad”"))
            ]
        );
        // One parameter the display names, with its range (Renoise's device-param).
        let (preview, applied) = answers(
            json!({"display": {"before": {"value": 0.5, "valueString": "0.000 dB"}}}),
            json!({"display": {"parameter": "Gain", "min": 0.0, "max": 1.4125, "before": {"value": 0.5, "valueString": "0.000 dB"}, "after": {"value": 1.0, "valueString": "6.021 dB"}}}),
        );
        let Some(ChangePicture::Values { values }) = params(&Answers { preview: &preview, applied: &applied }) else { panic!() };
        assert_eq!((values[0].name.as_str(), values[0].from, values[0].to, values[0].max), ("Gain", Some(0.5), Some(1.0), Some(1.4125)));
        assert_eq!((values[0].from_text.as_deref(), values[0].to_text.as_deref()), (Some("0.000 dB"), Some("6.021 dB")));
        // Renoise's own text below zero, and silence: the true minus (as titles write it).
        let (preview, applied) = answers(
            json!({}),
            json!({"display": {"parameter": "Gain", "before": {"value": 0.5, "valueString": "-6.021 dB"}, "after": {"value": 0.0, "valueString": "-INF dB"}}}),
        );
        let Some(ChangePicture::Values { values }) = params(&Answers { preview: &preview, applied: &applied }) else { panic!() };
        assert_eq!((values[0].from_text.as_deref(), values[0].to_text.as_deref()), (Some("−6.021 dB"), Some("−INF dB")));
        assert_eq!(minus_text("Key -12 (C-4)".into()), "Key −12 (C-4)", "a note name's hyphen stays");
        // REAPER's: drawn by the bridge, normalized with what the plug-in shows.
        let (preview, applied) = answers(
            json!({}),
            json!({"display": {"values": [{"name": "Freq", "from": 0.2, "to": 0.5, "min": 0.0, "max": 1.0, "fromText": "200 Hz", "toText": "1.2 kHz"}]}}),
        );
        let Some(ChangePicture::Values { values }) = params(&Answers { preview: &preview, applied: &applied }) else { panic!() };
        assert_eq!((values[0].from, values[0].to_text.as_deref()), (Some(0.2), Some("1.2 kHz")));
    }

    #[test]
    fn a_route_items_a_zone_and_settings_draw_what_moved() {
        let (preview, applied) = answers(
            json!({}),
            json!({"display": {"route": {"from": "Bass", "to": "Reverb", "change": "made", "values": [{"name": "Volume", "to": 0.0, "toText": "0 dB"}]}}}),
        );
        let Some(ChangePicture::Routes { routes }) = route(&Answers { preview: &preview, applied: &applied }) else { panic!() };
        assert_eq!((routes[0].from.as_str(), routes[0].to.as_str(), routes[0].change), ("Bass", "Reverb", RouteChange::Made));
        let item = |start: f64, at: &str| json!({"name": "Vox", "start": start, "length": 4.0, "fadeIn": 0.0, "fadeOut": 0.5, "at": at});
        let (preview, applied) =
            answers(json!({}), json!({"display": {"items": {"before": [item(2.0, "bar 2")], "after": [item(4.0, "bar 3")]}}}));
        let Some(ChangePicture::Items { before, after, values }) = items(&Answers { preview: &preview, applied: &applied }) else {
            panic!()
        };
        assert_eq!((before[0].start, after[0].start, after[0].fade_out), (2., 4., 0.5));
        assert_eq!(values.len(), 1, "only where it starts moved: {values:?}");
        assert_eq!((values[0].from_text.as_deref(), values[0].to_text.as_deref()), (Some("bar 2"), Some("bar 3")));
        let (preview, applied) = answers(
            json!({"display": {"before": {"noteRange": [36, 47], "baseNote": 36}}}),
            json!({"display": {"after": {"noteRange": [48, 59], "baseNote": 48}}}),
        );
        let Some(ChangePicture::Zones { before, after, values }) = zone(&Answers { preview: &preview, applied: &applied }) else {
            panic!()
        };
        assert_eq!((before[0].notes, after[0].notes, after[0].velocities, after[0].base), ([36, 47], [48, 59], None, Some(48)));
        let said: Vec<(&str, Option<&str>, Option<&str>)> =
            values.iter().map(|v| (v.name.as_str(), v.from_text.as_deref(), v.to_text.as_deref())).collect();
        assert_eq!(said, [("Keys", Some("C-3–B-3"), Some("C-4–B-4")), ("Base note", Some("C-3"), Some("C-4"))], "in Renoise's names");
        // A Drumkit layout: every zone.
        let zone = |low: u8, high: u8| json!({"noteRange": [low, high], "velocityRange": [0, 127], "baseNote": low});
        let (preview, applied) = answers(
            json!({"display": {"before": {"zones": [zone(0, 119), zone(0, 119), zone(36, 36)]}}}),
            json!({"display": {"after": {"zones": [zone(36, 36), zone(37, 37), zone(36, 36)]}}}),
        );
        let Some(ChangePicture::Zones { before, after, values }) = zone_picture(&preview, &applied) else { panic!() };
        assert_eq!((before.len(), after[1].notes, values[0].to_text.as_deref()), (3, [37, 37], Some("2 of 3 moved")));
        let (preview, applied) = answers(
            json!({"display": {"before": {"volumeDb": -6.0, "fineTune": 0, "loop": "off"}}}),
            json!({"display": {"after": {"volumeDb": 0.0, "fineTune": 0, "loop": "forward"}}}),
        );
        let Some(ChangePicture::Values { values }) = settings(&Answers { preview: &preview, applied: &applied }) else { panic!() };
        let said: Vec<(&str, Option<&str>, Option<&str>)> =
            values.iter().map(|v| (v.name.as_str(), v.from_text.as_deref(), v.to_text.as_deref())).collect();
        assert_eq!(said, [("Volume", Some("−6 dB"), Some("0 dB")), ("Loop", Some("off"), Some("forward"))], "the fine tune didn't move");
        assert_eq!((values[0].min, values[0].max), (Some(-60.), Some(12.)));
        // A chain loaded onto a track: the track's devices now.
        let (preview, applied) =
            answers(json!({"prior": {"devices": 1}}), json!({"applied": {"count": 3, "devices": ["EQ 5", "Compressor", "Reverb"]}}));
        let placement = chain(&Answers { preview: &preview, applied: &applied }).unwrap();
        assert_eq!(placement.devices.as_deref(), Some(&["EQ 5".to_string(), "Compressor".into(), "Reverb".into()][..]));
    }

    #[test]
    fn a_takes_notes_are_drawn_from_the_bar_the_first_is_in_and_a_lanes_events_as_points() {
        let note = |pitch: f64, start: f64| json!({"pitch": pitch, "start": start, "length": 0.5, "velocity": 90});
        // Notes deep in a take (bar 17): drawn from that bar; the preview drew 2 of the 5 it touched.
        let preview =
            json!({"display": {"count": 5, "before": [note(36., 65.), note(43., 66.)], "after": [note(38., 65.), note(43., 66.)]}});
        let Some(ChangePicture::Notes { length, before, after, partial }) = midi(&Answers { preview: &preview, applied: &json!({}) })
        else {
            panic!("notes")
        };
        assert_eq!((length, before[0].start, after[0].pitch, after[1].start, partial), (4., 1., 38., 2., true));
        // A pitch bend lane's events, centred on 0 as REAPER says them.
        let bend = |start: f64, value: i64| json!({"type": "pitchbend", "channel": 1, "value": value, "start": start});
        let preview = json!({"display": {"after": [bend(8., 0), bend(9., 4096), bend(10., -8192)]}});
        let picture = midi(&Answers { preview: &preview, applied: &json!({}) });
        assert_eq!(
            picture,
            Some(ChangePicture::Automation {
                parameter: "Pitch".into(),
                min: -8192.,
                max: 8191.,
                before: vec![],
                after: vec![[0., 0.], [1., 4096.], [2., -8192.]],
                words: None,
            })
        );
        // Events of two lanes: no one lane to draw.
        let cc = json!({"type": "cc", "channel": 1, "controller": 1, "value": 3, "start": 0.0});
        let preview = json!({"display": {"after": [cc, bend(1., 0)]}});
        assert_eq!(midi(&Answers { preview: &preview, applied: &json!({}) }), None);
    }
}
