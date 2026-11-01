//! Each change's picture (core's DAW-neutral `ChangePicture`), made from what Live's preview, the change asked
//! for and Live's answer already hold: the values that moved (what was asked beside what Live set), a channel's
//! levels, a clip's notes, an automation lane, a route, the transport's settings, the Arrangement's locators, a clip's
//! warp markers. An equaliser's or a compressor's curve needs the device's other parameters too: [`curve`] makes it
//! from them once they're read.

use super::summaries::{coalesce, label};
use super::*;
use crate::more_changes::{bars, span};
use kumi_types::picture::Dynamics;
use kumi_types::picture::EqBand;
use kumi_types::picture::EqShape;
use kumi_types::picture::MixLevels;
use kumi_types::picture::MixSend;
use kumi_types::picture::PictureMark;
use kumi_types::picture::PictureRoute;
use kumi_types::picture::PictureTrack;
use kumi_types::picture::PictureValue;
use kumi_types::picture::RouteChange;
use kumi_types::picture::TrackMark;

/// The picture of a change, from its preview, its input and Live's answer (none before it's applied), on its track
/// (by name, when it's on one).
pub(crate) fn picture(
    kind: &ChangeKind,
    preview: &JsonObject,
    input: &JsonObject,
    applied: Option<&JsonObject>,
    track: Option<&str>,
) -> Option<ChangePicture> {
    let empty = JsonObject::new();
    let applied = applied.unwrap_or(&empty);
    match kind.tool.as_str() {
        "set_tempo" => tempo(preview, input),
        "set_mixer" => mixer(preview, applied),
        "set_device_parameter" | "set_device_parameters" => {
            Some(ChangePicture::Values { values: parameter_values(preview, input, applied) })
                .filter(|p| !matches!(p, ChangePicture::Values { values } if values.is_empty()))
        }
        "write_midi_clip" => new_clip(preview, input),
        "change_notes" | "delete_notes" => edited_notes(kind, preview, input),
        "set_automation" => automation(preview, input, applied),
        "set_routing" => routing(preview, input, track),
        "set_transport" => transport(preview, input),
        "set_locators" => locators(preview, input),
        "delete_locator" => deleted_locator(preview),
        "set_warp_markers" => warp_markers(preview, input, applied),
        "set_sidechain" => device_input(preview, input, track),
        _ => None,
    }
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.into()
}

/// A track's input or output that moved, as a route (from where into the track, or from it to where), and its arm and
/// monitoring as values: on the first route, or alone.
fn routing(preview: &JsonObject, input: &JsonObject, track: Option<&str>) -> Option<ChangePicture> {
    let prior = record(preview.get("prior"));
    let proposed = if preview.get("proposed").is_some_and(Value::is_object) { record(preview.get("proposed")) } else { input };
    let track = track.unwrap_or("Track");
    // "Ext. In (1/2)": where, and the channel when it says more.
    let place = |side: &JsonObject, (kind, sub): (&str, &str)| {
        let name = label(side.get(kind))?;
        Some(match label(side.get(sub)).filter(|sub| *sub != name) {
            Some(sub) => format!("{name} ({sub})"),
            None => name,
        })
    };
    let mut after = prior.clone();
    for (key, value) in proposed {
        after.insert(key.clone(), value.clone());
    }
    let mut routes = Vec::new();
    for (ends, word, into) in [(("inputType", "inputSubRouting"), "Input", true), (("outputType", "outputSubRouting"), "Output", false)] {
        if !proposed.contains_key(ends.0) && !proposed.contains_key(ends.1) {
            continue;
        }
        let Some(to) = place(&after, ends) else { continue };
        let value = PictureValue { name: word.into(), from_text: place(prior, ends), to_text: Some(to.clone()), ..Default::default() };
        let (from, to) = if into { (to, track.to_owned()) } else { (track.to_owned(), to) };
        routes.push(PictureRoute { from, to, change: RouteChange::Changed, values: vec![value] });
    }
    let mut values = Vec::new();
    if let Some(arm) = proposed.get("arm").and_then(Value::as_bool) {
        values.push(PictureValue {
            name: "Arm".into(),
            from_text: prior.get("arm").and_then(Value::as_bool).map(on_off),
            to_text: Some(on_off(arm)),
            ..Default::default()
        });
    }
    if let Some(monitoring) = label(proposed.get("monitoring")) {
        // Live's states by number: 0 In, 1 Auto, 2 Off.
        let was = match prior.get("monitoring") {
            Some(Value::Number(state)) => state.as_f64().and_then(|n| ["In", "Auto", "Off"].get(n as usize).map(|s| s.to_string())),
            other => label(other).map(|text| capital_first(&text)),
        };
        values.push(PictureValue {
            name: "Monitoring".into(),
            from_text: was,
            to_text: Some(capital_first(&monitoring)),
            ..Default::default()
        });
    }
    match routes.first_mut() {
        Some(first) => {
            first.values.extend(values);
            Some(ChangePicture::Routes { routes })
        }
        None => (!values.is_empty()).then_some(ChangePicture::Values { values }),
    }
}

/// A device's input or sidechain set from another track: the route from there into the device's track, with what it
/// listened to before.
fn device_input(preview: &JsonObject, input: &JsonObject, track: Option<&str>) -> Option<ChangePicture> {
    let from = label(input.get("routingType"))?;
    let from = match label(input.get("routingChannel")).filter(|channel| *channel != from) {
        Some(channel) => format!("{from} ({channel})"),
        None => from,
    };
    let sidechain = input.get("action").and_then(Value::as_str) == Some("sidechain");
    let prior = record(preview.get("prior"));
    let value = PictureValue {
        name: if sidechain { "Sidechain" } else { "Input" }.into(),
        from_text: label(prior.get("routingType")),
        to_text: Some(from.clone()),
        ..Default::default()
    };
    let to = track.unwrap_or("Device").to_owned();
    Some(ChangePicture::Routes { routes: vec![PictureRoute { from, to, change: RouteChange::Changed, values: vec![value] }] })
}

fn capital_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// The transport's settings that moved, each before and after: the loop (where, or off), the metronome, punch in and
/// out, the playhead.
fn transport(preview: &JsonObject, input: &JsonObject) -> Option<ChangePicture> {
    let prior = record(preview.get("prior"));
    let proposed = if preview.get("proposed").is_some_and(Value::is_object) { record(preview.get("proposed")) } else { input };
    let mut values = Vec::new();
    let looped = record(prior.get("loop"));
    if ["loopEnabled", "loopStart", "loopLength"].iter().any(|key| proposed.contains_key(*key)) {
        let words = |on: bool, start: Option<f64>, length: Option<f64>| match (on, start, length) {
            (true, Some(start), Some(length)) => format!("{} for {}", bars(start), span(length)),
            (true, _, _) => "on".to_string(),
            _ => "off".to_string(),
        };
        let (was_on, was_start, was_length) =
            (looped.get("enabled") == Some(&json!(true)), finite(looped.get("start")), finite(looped.get("length")));
        let on = proposed.get("loopEnabled").and_then(Value::as_bool).unwrap_or(was_on);
        let (start, length) = (finite(proposed.get("loopStart")).or(was_start), finite(proposed.get("loopLength")).or(was_length));
        values.push(PictureValue {
            name: "Loop".into(),
            from_text: (!looped.is_empty()).then(|| words(was_on, was_start, was_length)),
            to_text: Some(words(on, start, length)),
            ..Default::default()
        });
    }
    for (key, name) in [("metronome", "Metronome"), ("punchIn", "Punch in"), ("punchOut", "Punch out")] {
        if let Some(on) = proposed.get(key).and_then(Value::as_bool) {
            let from_text = prior.get(key).and_then(Value::as_bool).map(on_off);
            values.push(PictureValue { name: name.into(), from_text, to_text: Some(on_off(on)), ..Default::default() });
        }
    }
    if let Some(position) = finite(proposed.get("position")) {
        values.push(PictureValue {
            name: "Playhead".into(),
            from_text: finite(prior.get("position")).map(bars),
            to_text: Some(bars(position)),
            ..Default::default()
        });
    }
    (!values.is_empty()).then_some(ChangePicture::Values { values })
}

/// A locator as Live's preview reads it: its name and where it is, in beats.
fn mark(row: &Value) -> Option<PictureMark> {
    let row = record(Some(row));
    Some(PictureMark { name: label(row.get("name")).unwrap_or_default(), at: finite(coalesce(row.get("position"), row.get("time")))? })
}

/// The Arrangement's locators before and after two new ones, with where the new ones are.
fn locators(preview: &JsonObject, input: &JsonObject) -> Option<ChangePicture> {
    let before: Vec<PictureMark> = array(preview.get("prior")).iter().filter_map(mark).collect();
    let made: Vec<PictureMark> = if preview.get("proposed").is_some_and(Value::is_array) {
        array(preview.get("proposed")).iter().filter_map(mark).collect()
    } else {
        [("startName", "start"), ("endName", "end")]
            .iter()
            .filter_map(|(name, at)| Some(PictureMark { name: label(input.get(*name)).unwrap_or_default(), at: finite(input.get(*at))? }))
            .collect()
    };
    if made.is_empty() {
        return None;
    }
    let values =
        made.iter().map(|mark| PictureValue { name: mark.name.clone(), to_text: Some(bars(mark.at)), ..Default::default() }).collect();
    let after = before.iter().cloned().chain(made).collect();
    Some(ChangePicture::Marks { before, after, values })
}

/// A locator taken away: where it was.
fn deleted_locator(preview: &JsonObject) -> Option<ChangePicture> {
    let gone = mark(coalesce(preview.get("locator"), preview.get("target"))?)?;
    let value = PictureValue {
        name: if gone.name.is_empty() { "Locator".into() } else { gone.name.clone() },
        from_text: Some(bars(gone.at)),
        to_text: Some("taken away".into()),
        ..Default::default()
    };
    Some(ChangePicture::Marks { before: vec![gone], after: Vec::new(), values: vec![value] })
}

/// A clip's warp markers before and after one was added, moved or taken away: as Live answered once it's applied, else
/// as the change will leave them (a new marker holds the moment the clip plays at its beat now, on the line through the
/// markers either side of it, as Live makes it).
fn warp_markers(preview: &JsonObject, input: &JsonObject, applied: &JsonObject) -> Option<ChangePicture> {
    let pairs = warp_pairs;
    let before = pairs(array(preview.get("markers")));
    let beat = finite(input.get("beatTime"))?;
    let action = label(coalesce(preview.get("action"), input.get("action"))).unwrap_or_default();
    let distance = finite(input.get("distance"));
    let near = |pairs: &[[f64; 2]]| pairs.iter().position(|pair| (pair[0] - beat).abs() <= 1e-9 * beat.abs().max(1.));
    let after = if applied.get("markers").is_some_and(Value::is_array) {
        pairs(array(applied.get("markers")))
    } else {
        let mut after = before.clone();
        match action.as_str() {
            "add" => after.push([beat, second_at(&before, beat)?]),
            "move" => {
                let at = near(&after)?;
                after[at][0] = beat + distance?;
            }
            "delete" => {
                after.remove(near(&after)?);
            }
            _ => return None,
        }
        after.sort_by(|a, b| a[0].total_cmp(&b[0]));
        after
    };
    let (from_text, to_text) = match action.as_str() {
        "add" => (None, Some(format!("new at {}", bars(beat)))),
        "move" => (Some(bars(beat)), distance.map(|distance| bars(beat + distance))),
        _ => (Some(bars(beat)), Some("taken away".to_string())),
    };
    Some(ChangePicture::Warp {
        before,
        after,
        values: vec![PictureValue { name: "Warp marker".into(), from_text, to_text, ..Default::default() }],
    })
}

/// Warp markers as Live reads them, each as `[beat, second]`, in the clip's order.
fn warp_pairs(rows: &[Value]) -> Vec<[f64; 2]> {
    let mut pairs: Vec<[f64; 2]> = rows
        .iter()
        .filter_map(|row| {
            let row = record(Some(row));
            Some([finite(row.get("beatTime"))?, finite(row.get("sampleTime"))?])
        })
        .collect();
    pairs.sort_by(|a, b| a[0].total_cmp(&b[0]));
    pairs
}

/// An audio clip's waveform: its sample's `seconds` and `peaks` (as read from its file), the part of the sample it
/// plays before the change and after (from `sample`, as the preview read the clip, with what the change set laid over
/// it), and the values that moved. A warped clip's loop is in beats: its warp markers say where those are in the sample.
pub fn audio_wave(seconds: f64, peaks: Vec<f64>, sample: &JsonObject, proposed: &JsonObject, values: Vec<PictureValue>) -> ChangePicture {
    let played = |side: &JsonObject| -> Option<[f64; 2]> {
        let from = finite(side.get("loopStart")).or_else(|| finite(side.get("startMarker")))?;
        let to = finite(side.get("loopEnd")).or_else(|| finite(side.get("endMarker")))?;
        if side.get("warping") != Some(&json!(true)) {
            return Some([from, to]);
        }
        let markers = warp_pairs(array(side.get("warpMarkers")));
        Some([second_at(&markers, from)?, second_at(&markers, to)?])
    };
    let mut after = sample.clone();
    for (key, value) in proposed {
        after.insert(key.clone(), value.clone());
    }
    ChangePicture::Wave { seconds, peaks, before: played(sample), after: played(&after), values }
}

/// An audio clip's settings that moved, before and after: its gain (in its range), transposition, detune, warping and
/// warp mode, fades.
pub fn audio_values(preview: &JsonObject, input: &JsonObject) -> Vec<PictureValue> {
    let prior = record(preview.get("prior"));
    let proposed = if preview.get("proposed").is_some_and(Value::is_object) { record(preview.get("proposed")) } else { input };
    let mut values = Vec::new();
    let number = |value: Option<&Value>, unit: &str, signed: bool| {
        finite(value).map(|n| format!("{}{}{unit}", if signed && n > 0. { "+" } else { "" }, format_number(n, None)))
    };
    if let Some(gain) = finite(proposed.get("gain")) {
        let from = finite(prior.get("gain"));
        values.push(PictureValue { name: "Gain".into(), from, to: Some(gain), min: Some(0.), max: Some(1.), ..Default::default() });
    }
    for (key, name, unit) in [("pitchCoarse", "Transpose", " st"), ("pitchFine", "Detune", " ct")] {
        if proposed.contains_key(key) {
            let (from_text, to_text) = (number(prior.get(key), unit, true), number(proposed.get(key), unit, true));
            values.push(PictureValue { name: name.into(), from_text, to_text, ..Default::default() });
        }
    }
    if let Some(on) = proposed.get("warping").and_then(Value::as_bool) {
        let from_text = prior.get("warping").and_then(Value::as_bool).map(on_off);
        values.push(PictureValue { name: "Warp".into(), from_text, to_text: Some(on_off(on)), ..Default::default() });
    }
    if proposed.contains_key("warpMode") {
        const MODES: [&str; 7] = ["Beats", "Tones", "Texture", "Re-Pitch", "Complex", "REX", "Complex Pro"];
        let mode = |value: Option<&Value>| finite(value).and_then(|n| MODES.get(n as usize)).map(|mode| mode.to_string());
        values.push(PictureValue {
            name: "Warp mode".into(),
            from_text: mode(prior.get("warpMode")),
            to_text: mode(proposed.get("warpMode")),
            ..Default::default()
        });
    }
    for (key, name) in [("fadeInLength", "Fade in"), ("fadeOutLength", "Fade out")] {
        if proposed.contains_key(key) {
            let (from_text, to_text) = (number(prior.get(key), "", false), number(proposed.get(key), "", false));
            values.push(PictureValue { name: name.into(), from_text, to_text, ..Default::default() });
        }
    }
    values
}

/// The Set's tracks after a change made some or took some away: `tracks` as they were, in order (each name and
/// colour), `made` the new ones at their places after it (each place and name), `removed` the places of those taken
/// away. None when it did neither.
pub fn track_list(tracks: &[(String, Option<String>)], made: &[(usize, String)], removed: &[usize]) -> Option<ChangePicture> {
    if made.is_empty() && removed.is_empty() {
        return None;
    }
    let mut list: Vec<PictureTrack> = tracks
        .iter()
        .enumerate()
        .map(|(at, (name, color))| PictureTrack {
            name: name.clone(),
            color: color.clone(),
            mark: removed.contains(&at).then_some(TrackMark::Removed),
        })
        .collect();
    let mut made = made.to_vec();
    made.sort_by_key(|(at, _)| *at);
    for (at, name) in made {
        list.insert(at.min(list.len()), PictureTrack { name, color: None, mark: Some(TrackMark::Made) });
    }
    Some(ChangePicture::Tracks { tracks: list, more: 0 })
}

/// The moment of the sample a clip plays at `beat`, on the line through the warp markers either side of it (or the
/// nearest two, past them): None with fewer than two.
fn second_at(markers: &[[f64; 2]], beat: f64) -> Option<f64> {
    if markers.len() < 2 {
        return None;
    }
    let right = markers.iter().position(|marker| beat <= marker[0]).unwrap_or(markers.len() - 1).max(1);
    let (left, right) = (markers[right - 1], markers[right]);
    (right[0] != left[0]).then(|| left[1] + (beat - left[0]) * (right[1] - left[1]) / (right[0] - left[0]))
}

fn tempo(preview: &JsonObject, input: &JsonObject) -> Option<ChangePicture> {
    let from = finite(preview.get("priorTempo"));
    let to = finite(preview.get("proposedTempo"))?;
    let bpm = |n: f64| format!("{} BPM", format_number(n, None));
    let asked = finite(input.get("tempo")).filter(|asked| (asked - to).abs() > 0.005).map(bpm);
    Some(ChangePicture::Values {
        values: vec![PictureValue {
            name: "Tempo".into(),
            from,
            to: Some(to),
            min: Some(20.),
            max: Some(999.),
            from_text: from.map(bpm),
            to_text: Some(bpm(to)),
            asked,
        }],
    })
}

fn mixer(preview: &JsonObject, applied: &JsonObject) -> Option<ChangePicture> {
    let levels = |values: &JsonObject, texts: &JsonObject| {
        let send_texts = array(texts.get("sends"));
        MixLevels {
            volume: finite(values.get("volume")),
            volume_text: label(texts.get("volume")),
            pan: finite(values.get("pan")),
            pan_text: label(texts.get("pan")),
            sends: array(values.get("sends"))
                .iter()
                .enumerate()
                .filter_map(|(index, level)| {
                    Some(MixSend {
                        name: String::from(char::from(b'A' + (index as u8).min(25))),
                        level: level.as_f64()?,
                        text: label(send_texts.get(index)),
                    })
                })
                .collect(),
            mute: values.get("mute").and_then(Value::as_bool),
            solo: values.get("solo").and_then(Value::as_bool),
        }
    };
    let prior = record(preview.get("prior"));
    let proposed = record(preview.get("proposed"));
    let prior_texts = record(preview.get("priorDisplay"));
    // What wasn't asked stays as it was: the after shows the whole channel. A shorter list of sends sets the first
    // ones; the rest stay.
    let mut after = prior.clone();
    for (key, value) in proposed {
        let merged = if key == "sends" { by_index(prior.get("sends"), Some(value)) } else { value.clone() };
        after.insert(key.clone(), merged);
    }
    // The words after: Live's own once it's applied; for what wasn't asked, the words before. An asked level never takes
    // its old words (an unconfirmed change shows its numbers only).
    let mut after_texts = JsonObject::new();
    for (key, value) in prior_texts {
        if key == "sends" {
            let asked = array(proposed.get("sends")).len();
            let kept = array(Some(value)).iter().enumerate().map(|(i, text)| if i < asked { Value::Null } else { text.clone() }).collect();
            after_texts.insert(key.clone(), Value::Array(kept));
        } else if !proposed.contains_key(key) {
            after_texts.insert(key.clone(), value.clone());
        }
    }
    for (key, value) in record(applied.get("display")) {
        let merged = if key == "sends" { by_index(after_texts.get("sends"), Some(value)) } else { value.clone() };
        after_texts.insert(key.clone(), merged);
    }
    Some(ChangePicture::Mix { before: levels(prior, prior_texts), after: levels(&after, &after_texts) })
}

/// A list laid over another by place: each of `over`'s items replaces the one at its place (a null keeps it).
fn by_index(under: Option<&Value>, over: Option<&Value>) -> Value {
    let mut items = array(under).to_vec();
    for (index, item) in array(over).iter().enumerate() {
        if item.is_null() {
            continue;
        }
        match items.get_mut(index) {
            Some(slot) => *slot = item.clone(),
            None => {
                items.resize(index, Value::Null);
                items.push(item.clone());
            }
        }
    }
    Value::Array(items)
}

/// The parameters that moved (all of them when none did): each in its range, as Live shows it before and after, and
/// what was asked when Live set something else.
pub(super) fn parameter_values(preview: &JsonObject, input: &JsonObject, applied: &JsonObject) -> Vec<PictureValue> {
    let rows: Vec<&JsonObject> = if preview.get("parameters").is_some_and(Value::is_array) {
        array(preview.get("parameters")).iter().map(|row| record(Some(row))).collect()
    } else {
        preview.get("parameter").map(|row| vec![record(Some(row))]).unwrap_or_default()
    };
    let moved: Vec<&JsonObject> =
        rows.iter().copied().filter(|row| finite(row.get("currentValue")) != finite(row.get("proposedValue"))).collect();
    let rows = if moved.is_empty() { rows } else { moved };
    let after_rows: Vec<&JsonObject> = array(applied.get("parameters")).iter().map(|row| record(Some(row))).collect();
    let asked_rows: Vec<&JsonObject> = if input.get("values").is_some_and(Value::is_array) {
        array(input.get("values")).iter().map(|row| record(Some(row))).collect()
    } else {
        vec![input]
    };
    let single = rows.len() == 1 && after_rows.is_empty();
    rows.iter()
        .map(|row| {
            let name = label(row.get("name")).unwrap_or_else(|| "Parameter".into());
            let to_text = if single {
                label(applied.get("displayValue"))
            } else {
                label(after_rows.iter().rev().find(|after| after.get("ref") == row.get("ref")).and_then(|after| after.get("displayValue")))
            };
            let asked = asked_rows
                .iter()
                .find(|asked| {
                    (asked.get("parameterRef").is_some() && asked.get("parameterRef") == row.get("ref"))
                        || label(asked.get("parameter")).is_some_and(|asked| asked.eq_ignore_ascii_case(&name))
                })
                .or_else(|| (single && asked_rows.len() == 1).then(|| &asked_rows[0]))
                .and_then(|asked| asked.get("value"));
            let to = finite(row.get("proposedValue"));
            PictureValue {
                asked: asked.and_then(|asked| asked_apart(asked, to, to_text.as_deref())),
                name,
                from: finite(row.get("currentValue")),
                to,
                min: finite(row.get("min")),
                max: finite(row.get("max")),
                from_text: label(row.get("displayValue")),
                to_text,
            }
        })
        .collect()
}

fn new_clip(preview: &JsonObject, input: &JsonObject) -> Option<ChangePicture> {
    let proposed = record(preview.get("proposed"));
    let source = if proposed.get("notes").is_some_and(Value::is_array) { array(proposed.get("notes")) } else { array(input.get("notes")) };
    let notes = summaries::drawn_notes(source);
    let length = finite(coalesce(proposed.get("length"), input.get("length"))).filter(|n| *n > 0.)?;
    (!notes.is_empty()).then(|| ChangePicture::Notes { length, before: Vec::new(), after: notes, partial: false })
}

/// Notes changed or deleted: the ones touched, before (Live's own) and after (with the changes laid over them).
fn edited_notes(kind: &ChangeKind, preview: &JsonObject, input: &JsonObject) -> Option<ChangePicture> {
    let prior = array(preview.get("priorNotes"));
    if prior.is_empty() {
        return None;
    }
    let before = summaries::drawn_notes(prior);
    let after = if kind.tool == "delete_notes" {
        Vec::new()
    } else {
        let changes =
            if preview.get("patches").is_some_and(Value::is_array) { array(preview.get("patches")) } else { array(input.get("notes")) };
        prior
            .iter()
            .map(|note| {
                let mut note = record(Some(note)).clone();
                // Live's notes and the patches on them are keyed by `id`.
                let id = |row: &JsonObject| row.get("id").or_else(|| row.get("noteId")).cloned();
                if let Some(change) =
                    changes.iter().map(|change| record(Some(change))).find(|change| id(change).is_some() && id(change) == id(&note))
                {
                    for key in ["pitch", "start", "duration", "velocity"] {
                        if let Some(value) = change.get(key).filter(|v| v.is_number()) {
                            note.insert(key.into(), value.clone());
                        }
                    }
                }
                Value::Object(note)
            })
            .collect::<Vec<_>>()
    };
    let after = summaries::drawn_notes(&after);
    let end = before.iter().chain(&after).map(|note| note.start + note.duration).fold(0., f64::max);
    Some(ChangePicture::Notes { length: (end / 4.).ceil().max(1.) * 4., before, after, partial: false })
}

/// A clip's automation: Live's points before; after, as Live read them once it's applied (a bridge that doesn't say
/// leaves them to be worked out: those with what was drawn laid over them, or erased). Named, on its parameter's range,
/// as the preview read the parameter.
fn automation(preview: &JsonObject, input: &JsonObject, applied: &JsonObject) -> Option<ChangePicture> {
    let mut picture = drawn_automation(preview, input, applied)?;
    let parameter = record(preview.get("parameter"));
    if let (Some(name), Some(min), Some(max)) = (label(parameter.get("name")), finite(parameter.get("min")), finite(parameter.get("max"))) {
        name_lane(&mut picture, &name, min, max);
    }
    Some(picture)
}

fn drawn_automation(preview: &JsonObject, input: &JsonObject, applied: &JsonObject) -> Option<ChangePicture> {
    let points = |items: &[Value]| {
        items
            .iter()
            .filter_map(|point| {
                let point = record(Some(point));
                Some([finite(coalesce(point.get("time"), point.get("start")))?, finite(point.get("value"))?])
            })
            .collect::<Vec<[f64; 2]>>()
    };
    let before = points(array(record(preview.get("current")).get("points")));
    let action = label(coalesce(preview.get("action"), input.get("action"))).unwrap_or_default();
    let envelope = record(applied.get("envelope"));
    let after = match action.as_str() {
        "insert" | "insert-step" | "delete-range" | "delete-envelope" if envelope.get("exists") == Some(&json!(false)) => Vec::new(),
        "insert" | "insert-step" | "delete-range" | "delete-envelope" if envelope.get("points").is_some_and(Value::is_array) => {
            points(array(envelope.get("points")))
        }
        "insert" => {
            let drawn = points(array(input.get("points")));
            let (first, last) =
                (drawn.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), drawn.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max));
            before.iter().copied().filter(|p| p[0] < first || p[0] > last).chain(drawn).collect()
        }
        "insert-step" => {
            let (start, length, value) = (finite(input.get("start"))?, finite(input.get("length"))?, finite(input.get("value"))?);
            let outside = before.iter().copied().filter(|p| p[0] < start || p[0] > start + length);
            outside.chain([[start, value], [start + length, value]]).collect()
        }
        "delete-range" => {
            let (from, to) = (finite(input.get("from"))?, finite(input.get("to"))?);
            before.iter().copied().filter(|p| p[0] < from || p[0] > to).collect()
        }
        "delete-envelope" => Vec::new(),
        _ => return None,
    };
    let values = before.iter().chain(&after).map(|p| p[1]);
    let (low, high) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), v| (low.min(v), high.max(v)));
    if !low.is_finite() {
        return None;
    }
    // Most of Live's parameters run 0–1: those keep that range, so a small move looks small.
    let (min, max) = if low >= 0. && high <= 1. {
        (0., 1.)
    } else if high > low {
        (low, high)
    } else {
        (low - 1., high + 1.)
    };
    Some(ChangePicture::Automation { parameter: String::new(), min, max, before, after, words: None })
}

pub use kumi_types::picture::{amount, asked_apart, name_lane, CurveReader, ParameterRow};

/// One of a Live device's parameters, as the bridge reads it.
pub fn parameter_row(row: &JsonObject) -> Option<ParameterRow> {
    Some(ParameterRow {
        name: label(row.get("name"))?,
        value: finite(row.get("value"))?,
        min: finite(row.get("min")).unwrap_or(0.),
        max: finite(row.get("max")).unwrap_or(1.),
        text: label(coalesce(row.get("displayValue"), row.get("display"))),
    })
}

/// Live's equalisers (EQ Eight, Channel EQ) and compressors, by their parameters' names.
pub struct LiveCurves;

impl CurveReader for LiveCurves {
    fn eq_bands(&self, rows: &[ParameterRow]) -> Option<Vec<EqBand>> {
        eq_bands(rows)
    }
    fn dynamics(&self, rows: &[ParameterRow]) -> Option<Dynamics> {
        dynamics(rows)
    }
}

/// Each change's curve on a Live device (core's `curves`), as the unit tests below draw it.
#[cfg(test)]
pub fn curves(read: &[ParameterRow], changes: &[(String, Vec<PictureValue>)]) -> Vec<(String, ChangePicture)> {
    kumi_types::picture::curves(&LiveCurves, read, changes)
}

/// The curves one read of a Live device gives the changes it was for (core's `curves_for`).
pub fn curves_for(
    read: &[ParameterRow],
    ours: &[(String, Vec<PictureValue>)],
    newer: &[(String, Vec<PictureValue>)],
    applied: impl Fn(&str) -> bool,
) -> Vec<(String, ChangePicture)> {
    kumi_types::picture::curves_for(&LiveCurves, read, ours, newer, applied)
}

/// Whether a parameter of this name may be an equaliser's or a compressor's: worth reading its device's others for
/// the curve (the device's own names then say whether it is one).
pub fn curve_parameter(name: &str) -> bool {
    static BAND: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"^[1-8] (Filter On|Filter Type|Frequency|Gain|Q|Resonance) A$").expect("regex"));
    BAND.is_match(name)
        || ["Scale", "Low Gain", "Mid Gain", "Mid Freq", "High Gain", "Highpass On", "Threshold", "Ratio", "Knee", "Output Gain", "Makeup"]
            .contains(&name)
}

/// EQ Eight's bands (the A channel), or Channel EQ's three, by their parameters.
pub fn eq_bands(rows: &[ParameterRow]) -> Option<Vec<EqBand>> {
    let find = |name: &str| rows.iter().find(|row| row.name == name);
    // Live's own words say what it is: an EQ's frequencies are in Hz (a Rack's macros named alike show plain numbers).
    let in_hz = |row: &ParameterRow| row.text.as_deref().is_some_and(|text| text.to_ascii_lowercase().ends_with("hz"));
    let frequencies: Vec<&ParameterRow> = rows.iter().filter(|row| row.name.ends_with(" Frequency A")).collect();
    if !frequencies.is_empty() && frequencies.iter().all(|row| in_hz(row)) {
        // Output gain aside, Scale scales every band's gain (100 % is as set).
        let scale = find("Scale").map(|row| row.value).filter(|v| v.is_finite()).unwrap_or(1.);
        let bands = (1..=8)
            .filter_map(|band| {
                let get = |what: &str| find(&format!("{band} {what} A"));
                let frequency = get("Frequency")?;
                let freq = frequency.text.as_deref().and_then(amount).unwrap_or_else(|| 10. * 2200f64.powf(position(frequency)));
                // Live 11 names it Resonance.
                let q = get("Q")
                    .or_else(|| get("Resonance"))
                    .map(|q| q.text.as_deref().and_then(amount).unwrap_or_else(|| 0.1 * 180f64.powf(position(q))))
                    .unwrap_or(0.71);
                let gain_db = get("Gain").map(|g| g.text.as_deref().and_then(amount).unwrap_or(g.value)).unwrap_or(0.) * scale;
                let kind = get("Filter Type");
                let kind = kind.and_then(|k| k.text.clone()).unwrap_or_else(|| {
                    ["High Pass 48dB", "High Pass 12dB", "Low Shelf", "Bell", "Notch", "High Shelf", "Low Pass 12dB", "Low Pass 48dB"]
                        [kind.map_or(3, |k| k.value.round().clamp(0., 7.) as usize)]
                    .into()
                });
                let (shape, slope) = shape_of(&kind)?;
                let on = get("Filter On").is_none_or(|on| on.value >= 0.5);
                Some(EqBand { shape, freq, gain_db, q, on, slope })
            })
            .collect::<Vec<_>>();
        return (!bands.is_empty()).then_some(bands);
    }
    if let (Some(low), Some(mid), Some(mid_freq), Some(high)) =
        (find("Low Gain"), find("Mid Gain"), find("Mid Freq").filter(|row| in_hz(row)), find("High Gain"))
    {
        let db = |row: &ParameterRow| row.text.as_deref().and_then(amount).unwrap_or(row.value);
        let mut bands = vec![
            EqBand { shape: EqShape::LowShelf, freq: 100., gain_db: db(low), q: 0.71, on: true, slope: None },
            EqBand {
                shape: EqShape::Bell,
                freq: mid_freq.text.as_deref().and_then(amount).unwrap_or(1000.),
                gain_db: db(mid),
                q: 0.71,
                on: true,
                slope: None,
            },
            EqBand { shape: EqShape::HighShelf, freq: 8000., gain_db: db(high), q: 0.71, on: true, slope: None },
        ];
        if find("Highpass On").is_some_and(|on| on.value >= 0.5) {
            bands.insert(0, EqBand { shape: EqShape::LowCut, freq: 80., gain_db: 0., q: 0.71, on: true, slope: Some(12.) });
        }
        return Some(bands);
    }
    None
}

/// A band's kind by Live's name for it.
fn shape_of(kind: &str) -> Option<(EqShape, Option<f64>)> {
    let kind = kind.to_ascii_lowercase();
    let slope = amount(&kind).filter(|n| *n > 0.);
    Some(if kind.contains("high pass") {
        (EqShape::LowCut, slope.or(Some(12.)))
    } else if kind.contains("low pass") {
        (EqShape::HighCut, slope.or(Some(12.)))
    } else if kind.contains("low shelf") {
        (EqShape::LowShelf, None)
    } else if kind.contains("high shelf") {
        (EqShape::HighShelf, None)
    } else if kind.contains("notch") {
        (EqShape::Notch, None)
    } else if kind.contains("bell") {
        (EqShape::Bell, None)
    } else {
        return None;
    })
}

/// How far along its range a value is, 0–1.
fn position(row: &ParameterRow) -> f64 {
    if row.max > row.min {
        ((row.value - row.min) / (row.max - row.min)).clamp(0., 1.)
    } else {
        0.
    }
}

/// A compressor's curve by its Threshold, Ratio, Knee and output (or makeup) gain.
pub fn dynamics(rows: &[ParameterRow]) -> Option<Dynamics> {
    let find = |name: &str| rows.iter().find(|row| row.name == name);
    let db = |row: &ParameterRow| row.text.as_deref().and_then(amount).unwrap_or(row.value);
    // A compressor's threshold reads in dB and its ratio as "4.00 : 1" (or a Glue's 2, 4 or 10).
    let threshold = find("Threshold").filter(|row| row.text.as_deref().is_some_and(|text| text.to_ascii_lowercase().contains("db")))?;
    let ratio = find("Ratio").filter(|row| {
        row.text.as_deref().is_some_and(|text| {
            text.contains(':') || text.to_ascii_lowercase().starts_with("inf") || matches!(text.trim(), "2" | "4" | "10")
        })
    })?;
    let ratio = match ratio.text.as_deref() {
        Some(text) if text.to_ascii_lowercase().starts_with("inf") => 100.,
        Some(text) => amount(text).unwrap_or(ratio.value),
        None => ratio.value,
    };
    Some(Dynamics {
        threshold_db: db(threshold),
        ratio: ratio.max(1.),
        knee_db: find("Knee").map(db).unwrap_or(0.),
        makeup_db: find("Output Gain").or_else(|| find("Makeup")).map(db).unwrap_or(0.),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A change's picture, on no track.
    fn picture(kind: &ChangeKind, preview: &JsonObject, input: &JsonObject, applied: Option<&JsonObject>) -> Option<ChangePicture> {
        super::picture(kind, preview, input, applied, None)
    }

    fn kind(tool: &str) -> &'static ChangeKind {
        CHANGES.iter().find(|kind| kind.tool == tool).unwrap()
    }

    fn object(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    fn row(name: &str, value: f64, text: &str) -> ParameterRow {
        ParameterRow { name: name.into(), value, min: 0., max: 1., text: Some(text.into()) }
    }

    #[test]
    fn amounts_are_read_from_what_live_shows() {
        assert_eq!(amount("-7.50 dB"), Some(-7.5));
        assert_eq!(amount("3.21 kHz"), Some(3210.));
        assert_eq!(amount("469 Hz"), Some(469.));
        assert_eq!(amount("4.00 : 1"), Some(4.));
        assert_eq!(amount("18.0"), Some(18.));
        assert_eq!(amount("-inf dB"), None);
        assert_eq!(amount("Bell"), None);
    }

    #[test]
    fn a_parameter_change_keeps_what_was_asked_beside_what_live_set() {
        let preview = object(json!({"device":{"name":"EQ Eight"},"parameters":[
            {"ref":"p1","name":"1 Gain A","min":-15,"max":15,"currentValue":0,"proposedValue":-2.5,"displayValue":"0.00 dB"},
            {"ref":"p2","name":"1 Q A","min":0,"max":1,"currentValue":0.38,"proposedValue":0.38,"displayValue":"0.71"}]}));
        let input = object(json!({"deviceRef":"d","values":[{"parameterRef":"p1","value":"-2 dB"},{"parameterRef":"p2","value":0.38}]}));
        let applied = object(json!({"parameters":[{"ref":"p1","displayValue":"-2.50 dB"}]}));
        let ChangePicture::Values { values } = picture(kind("set_device_parameters"), &preview, &input, Some(&applied)).unwrap() else {
            panic!()
        };
        assert_eq!(values.len(), 1, "only what moved");
        let gain = &values[0];
        assert_eq!((gain.name.as_str(), gain.from, gain.to, gain.min, gain.max), ("1 Gain A", Some(0.), Some(-2.5), Some(-15.), Some(15.)));
        assert_eq!((gain.from_text.as_deref(), gain.to_text.as_deref()), (Some("0.00 dB"), Some("-2.50 dB")));
        assert_eq!(gain.asked.as_deref(), Some("-2 dB"), "Live set -2.50 dB where -2 dB was asked");
        // Asked as Live shows it: nothing to add.
        let input = object(json!({"deviceRef":"d","values":[{"parameterRef":"p1","value":"-2.5 dB"}]}));
        let ChangePicture::Values { values } = picture(kind("set_device_parameters"), &preview, &input, Some(&applied)).unwrap() else {
            panic!()
        };
        assert_eq!(values[0].asked, None);
    }

    #[test]
    fn a_tempo_and_a_channel_draw_their_before_and_after() {
        let preview = object(json!({"priorTempo":120,"proposedTempo":124}));
        let ChangePicture::Values { values } = picture(kind("set_tempo"), &preview, &object(json!({"tempo":124})), None).unwrap() else {
            panic!()
        };
        assert_eq!(
            (values[0].from_text.as_deref(), values[0].to_text.as_deref(), values[0].asked.as_deref()),
            (Some("120 BPM"), Some("124 BPM"), None)
        );
        let preview = object(json!({"prior":{"volume":0.85,"pan":0,"sends":[0.2,0]},"proposed":{"volume":0.7},
            "priorDisplay":{"volume":"0.0 dB","pan":"C","sends":["-24 dB","-inf dB"]}}));
        let applied = object(json!({"display":{"volume":"-4.5 dB"}}));
        let ChangePicture::Mix { before, after } = picture(kind("set_mixer"), &preview, &object(json!({})), Some(&applied)).unwrap() else {
            panic!()
        };
        assert_eq!((before.volume, after.volume), (Some(0.85), Some(0.7)));
        assert_eq!((before.volume_text.as_deref(), after.volume_text.as_deref()), (Some("0.0 dB"), Some("-4.5 dB")));
        assert_eq!(after.pan, Some(0.), "what wasn't asked stays");
        assert_eq!(after.sends.len(), 2);
        assert_eq!((after.sends[0].name.as_str(), after.sends[0].level), ("A", 0.2));
    }

    #[test]
    fn notes_written_and_edited_draw_before_and_after() {
        let preview = object(json!({"proposed":{"length":4,"notes":[{"pitch":60,"start":0,"duration":1,"velocity":100}]}}));
        let ChangePicture::Notes { length, before, after, .. } =
            picture(kind("write_midi_clip"), &preview, &object(json!({})), None).unwrap()
        else {
            panic!()
        };
        assert_eq!((length, before.len(), after.len()), (4., 0, 1));
        let preview = object(
            json!({"priorNotes":[{"id":1,"pitch":60,"start":0,"duration":1,"velocity":100},{"id":2,"pitch":64,"start":1,"duration":1,"velocity":90}],
            "patches":[{"id":2,"pitch":65}]}),
        );
        let ChangePicture::Notes { before, after, .. } = picture(kind("change_notes"), &preview, &object(json!({})), None).unwrap() else {
            panic!()
        };
        assert_eq!(before.iter().map(|n| n.pitch).collect::<Vec<_>>(), [60., 64.]);
        assert_eq!(after.iter().map(|n| n.pitch).collect::<Vec<_>>(), [60., 65.]);
        let ChangePicture::Notes { after, .. } = picture(kind("delete_notes"), &preview, &object(json!({})), None).unwrap() else {
            panic!()
        };
        assert!(after.is_empty());
    }

    #[test]
    fn drawn_automation_lands_over_what_was_there() {
        let preview = object(
            json!({"action":"insert","current":{"exists":true,"points":[{"time":0,"value":0.2},{"time":2,"value":0.4},{"time":8,"value":0.6}]}}),
        );
        let input = object(json!({"action":"insert","points":[{"time":1,"value":1},{"time":3,"value":0}]}));
        let ChangePicture::Automation { min, max, before, after, .. } = picture(kind("set_automation"), &preview, &input, None).unwrap()
        else {
            panic!()
        };
        assert_eq!((min, max), (0., 1.));
        assert_eq!(before.len(), 3);
        assert_eq!(after, vec![[0., 0.2], [8., 0.6], [1., 1.], [3., 0.]], "the drawn span replaces what was in it");
    }

    #[test]
    fn eq_eights_bands_come_from_its_parameters() {
        let mut rows = vec![row("Scale", 1., "100 %")];
        for band in 1..=8 {
            rows.push(row(&format!("{band} Filter On A"), if band == 3 { 1. } else { 0. }, if band == 3 { "On" } else { "Off" }));
            rows.push(row(&format!("{band} Filter Type A"), 3., "Bell"));
            rows.push(row(&format!("{band} Frequency A"), 0.5, "469 Hz"));
            rows.push(ParameterRow { name: format!("{band} Gain A"), value: 0., min: -15., max: 15., text: Some("0.00 dB".into()) });
            rows.push(row(&format!("{band} Q A"), 0.5, "1.34"));
        }
        assert!(curve_parameter("3 Gain A") && curve_parameter("Threshold") && !curve_parameter("Dry/Wet"));
        let moved = vec![PictureValue {
            name: "3 Gain A".into(),
            from: Some(0.),
            to: Some(-4.),
            from_text: Some("0.00 dB".into()),
            to_text: Some("-4.00 dB".into()),
            ..Default::default()
        }];
        let gain = rows.iter_mut().find(|r| r.name == "3 Gain A").unwrap();
        gain.value = -4.;
        gain.text = Some("-4.00 dB".into());
        let drawn = curves(&rows, &[("c1".into(), moved.clone())]);
        let [(id, ChangePicture::Eq { before, after, values })] = drawn.as_slice() else { panic!() };
        assert_eq!(id, "c1");
        assert_eq!(after.len(), 8);
        let band = &after[2];
        assert_eq!((band.shape, band.freq, band.gain_db, band.q, band.on), (EqShape::Bell, 469., -4., 1.34, true));
        assert_eq!(before[2].gain_db, 0., "the moved parameter's prior gives the before");
        assert!(!after[0].on);
        assert_eq!(values, &moved);
        // Without Live's words for the frequencies, it isn't known to be an EQ: no curve.
        let bare: Vec<ParameterRow> = rows.iter().map(|r| ParameterRow { text: None, ..r.clone() }).collect();
        assert_eq!(eq_bands(&bare), None);
        // With them, a Q without words comes from its place in its range.
        let no_q: Vec<ParameterRow> =
            rows.iter().map(|r| if r.name.ends_with(" Q A") { ParameterRow { text: None, ..r.clone() } } else { r.clone() }).collect();
        let bands = eq_bands(&no_q).unwrap();
        assert!((bands[2].q - 1.34).abs() < 0.01, "{}", bands[2].q);
    }

    #[test]
    fn a_compressors_curve_comes_from_its_parameters() {
        let rows = vec![
            ParameterRow { name: "Threshold".into(), value: 0.6, min: 0., max: 1., text: Some("-20.0 dB".into()) },
            ParameterRow { name: "Ratio".into(), value: 0.75, min: 0., max: 1., text: Some("4.00 : 1".into()) },
            ParameterRow { name: "Knee".into(), value: 6., min: 0., max: 18., text: Some("6.0 dB".into()) },
            ParameterRow { name: "Output Gain".into(), value: 3., min: -36., max: 36., text: Some("3.00 dB".into()) },
        ];
        assert_eq!(dynamics(&rows), Some(Dynamics { threshold_db: -20., ratio: 4., knee_db: 6., makeup_db: 3. }));
        let limiting = [ParameterRow { text: Some("inf : 1".into()), ..rows[1].clone() }];
        assert_eq!(dynamics(&[rows[0].clone(), limiting[0].clone()]).unwrap().ratio, 100.);
    }

    #[test]
    fn a_run_of_changes_on_one_device_each_get_their_own_before_and_after() {
        let gain = |value: f64| ParameterRow { name: "Threshold".into(), value, min: -60., max: 0., text: Some(format!("{value:.1} dB")) };
        let ratio = ParameterRow { name: "Ratio".into(), value: 4., min: 1., max: 100., text: Some("4.00 : 1".into()) };
        let moved = |from: f64, to: f64| {
            vec![PictureValue {
                name: "Threshold".into(),
                from: Some(from),
                to: Some(to),
                from_text: Some(format!("{from:.1} dB")),
                to_text: Some(format!("{to:.1} dB")),
                ..Default::default()
            }]
        };
        // -10 → -20, then -20 → -30, read once after both.
        let read = [gain(-30.), ratio];
        let drawn = curves(&read, &[("c1".into(), moved(-10., -20.)), ("c2".into(), moved(-20., -30.))]);
        let threshold = |picture: &ChangePicture| match picture {
            ChangePicture::Dynamics { before, after, .. } => (before.threshold_db, after.threshold_db),
            _ => panic!(),
        };
        assert_eq!(drawn.len(), 2);
        assert_eq!((drawn[0].0.as_str(), threshold(&drawn[0].1)), ("c1", (-10., -20.)), "the first change's after isn't the later state");
        assert_eq!((drawn[1].0.as_str(), threshold(&drawn[1].1)), ("c2", (-20., -30.)));
    }

    #[test]
    fn one_parameter_draws_its_value_as_asked_and_as_set() {
        let preview = object(
            json!({"device":{"name":"Auto Filter"},"parameter":{"ref":"p","name":"Frequency","min":20,"max":20000,"currentValue":1000,"proposedValue":2000,"displayValue":"1.00 kHz"}}),
        );
        let input = object(json!({"deviceRef":"d","parameterRef":"p","value":2010}));
        let applied = object(json!({"displayValue":"2.00 kHz"}));
        let ChangePicture::Values { values } = picture(kind("set_device_parameter"), &preview, &input, Some(&applied)).unwrap() else {
            panic!()
        };
        let value = &values[0];
        assert_eq!(
            (value.name.as_str(), value.from, value.to, value.min, value.max),
            ("Frequency", Some(1000.), Some(2000.), Some(20.), Some(20000.))
        );
        assert_eq!(
            (value.from_text.as_deref(), value.to_text.as_deref(), value.asked.as_deref()),
            (Some("1.00 kHz"), Some("2.00 kHz"), Some("2010"))
        );
    }

    #[test]
    fn a_lane_takes_its_parameters_name_and_range_when_its_points_fit() {
        let drawn = object(json!({"action":"insert","points":[{"time":0,"value":0},{"time":4,"value":-8}]}));
        let mut lane = picture(kind("set_automation"), &JsonObject::new(), &drawn, None).unwrap();
        let range = |lane: &ChangePicture| match lane {
            ChangePicture::Automation { parameter, min, max, .. } => (parameter.clone(), *min, *max),
            _ => panic!(),
        };
        // Without a read of the parameter, the lane is drawn to its own points.
        assert_eq!(range(&lane), (String::new(), -8., 0.));
        name_lane(&mut lane, "3 Gain A", -15., 15.);
        assert_eq!(range(&lane), ("3 Gain A".into(), -15., 15.));
        // A range the points don't fit isn't this lane's: the name is still told.
        let mut lane = picture(kind("set_automation"), &JsonObject::new(), &drawn, None).unwrap();
        name_lane(&mut lane, "Dry/Wet", 0., 1.);
        assert_eq!(range(&lane), ("Dry/Wet".into(), -8., 0.));
    }

    #[test]
    fn automation_steps_erasures_and_removed_lanes_draw_what_is_left() {
        let current = json!({"exists":true,"points":[{"time":0,"value":0.2},{"time":4,"value":0.4},{"time":8,"value":0.6}]});
        let step = object(json!({"action":"insert-step","start":3,"length":2,"value":0.9}));
        let ChangePicture::Automation { after, .. } =
            picture(kind("set_automation"), &object(json!({"current":current})), &step, None).unwrap()
        else {
            panic!()
        };
        assert_eq!(after, vec![[0., 0.2], [8., 0.6], [3., 0.9], [5., 0.9]]);
        let erase = object(json!({"action":"delete-range","from":2,"to":6}));
        let ChangePicture::Automation { after, .. } =
            picture(kind("set_automation"), &object(json!({"current":current})), &erase, None).unwrap()
        else {
            panic!()
        };
        assert_eq!(after, vec![[0., 0.2], [8., 0.6]]);
        let remove = object(json!({"action":"delete-envelope"}));
        let ChangePicture::Automation { before, after, .. } =
            picture(kind("set_automation"), &object(json!({"current":current})), &remove, None).unwrap()
        else {
            panic!()
        };
        assert_eq!((before.len(), after.len()), (3, 0));
        // A new, empty lane has nothing to draw.
        let empty = object(json!({"current":{"exists":false,"points":[]}}));
        assert_eq!(picture(kind("set_automation"), &empty, &object(json!({"action":"create-envelope"})), None), None);
    }

    /// EQ Eight's parameters with bands 1–5 on as bells at 1 kHz, -6 dB (what a read after the change shows).
    fn eq_eight_after() -> Vec<ParameterRow> {
        let mut rows = vec![row("Scale", 1., "100 %")];
        for band in 1..=8 {
            let on = band <= 5;
            rows.push(row(&format!("{band} Filter On A"), if on { 1. } else { 0. }, if on { "On" } else { "Off" }));
            rows.push(row(&format!("{band} Filter Type A"), 3., "Bell"));
            rows.push(row(&format!("{band} Frequency A"), 0.6, "1.00 kHz"));
            rows.push(ParameterRow {
                name: format!("{band} Gain A"),
                value: if on { -6. } else { 0. },
                min: -15.,
                max: 15.,
                text: Some(if on { "-6.00 dB" } else { "0.00 dB" }.into()),
            });
            rows.push(row(&format!("{band} Q A"), 0.5, "1.34"));
        }
        rows
    }

    #[test]
    fn a_change_to_more_values_than_a_record_keeps_walks_back_all_of_them() {
        // Bands 1–5 turned on and cut: 20 values, more than a record's picture keeps.
        let mut moved = Vec::new();
        for band in 1..=5 {
            moved.push(PictureValue {
                name: format!("{band} Filter On A"),
                from: Some(0.),
                to: Some(1.),
                from_text: Some("Off".into()),
                to_text: Some("On".into()),
                ..Default::default()
            });
            moved.push(PictureValue {
                name: format!("{band} Gain A"),
                from: Some(0.),
                to: Some(-6.),
                from_text: Some("0.00 dB".into()),
                to_text: Some("-6.00 dB".into()),
                ..Default::default()
            });
            moved.push(PictureValue {
                name: format!("{band} Frequency A"),
                from: Some(0.6),
                to: Some(0.6),
                from_text: Some("1.00 kHz".into()),
                to_text: Some("1.00 kHz".into()),
                ..Default::default()
            });
            moved.push(PictureValue {
                name: format!("{band} Q A"),
                from: Some(0.5),
                to: Some(0.5),
                from_text: Some("1.34".into()),
                to_text: Some("1.34".into()),
                ..Default::default()
            });
        }
        assert_eq!(moved.len(), 20);
        let drawn = curves(&eq_eight_after(), &[("c1".into(), moved)]);
        let [(_, ChangePicture::Eq { before, after, .. })] = drawn.as_slice() else { panic!() };
        assert!(before.iter().all(|band| !band.on && band.gain_db == 0.), "all five were off at 0 dB before: {before:?}");
        assert!(after[..5].iter().all(|band| band.on && band.gain_db == -6.));
    }

    #[test]
    fn a_read_draws_only_the_changes_still_applied_and_only_on_their_own_device() {
        let gain = |band: u8, from: f64, to: f64| {
            vec![PictureValue {
                name: format!("{band} Gain A"),
                from: Some(from),
                to: Some(to),
                from_text: Some(format!("{from:.2} dB")),
                to_text: Some(format!("{to:.2} dB")),
                ..Default::default()
            }]
        };
        let read = eq_eight_after();
        let ours = [("c1".to_string(), gain(1, 0., -6.)), ("c2".to_string(), gain(2, 0., -6.))];
        // c2 was undone while the read waited: it isn't in what was read, and gets no curve.
        let drawn = curves_for(&read, &ours, &[], |id| id != "c2");
        assert_eq!(drawn.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["c1"]);
        // A change made while it read is walked back first, and draws from its own read.
        let newer = [("c3".to_string(), gain(3, 0., -6.))];
        let drawn = curves_for(&read, &ours[..1], &newer, |_| true);
        let [(id, ChangePicture::Eq { after, .. })] = drawn.as_slice() else { panic!() };
        assert_eq!((id.as_str(), after[2].gain_db), ("c1", 0.), "band 3's cut came after c1");
        // A moved parameter missing from the read: another device sits in that place now.
        let elsewhere = [("c4".to_string(), vec![PictureValue { name: "Drive".into(), to: Some(1.), ..Default::default() }])];
        assert!(curves_for(&read, &elsewhere, &[], |_| true).is_empty());
    }

    #[test]
    fn a_racks_macros_named_like_an_eq_or_a_compressor_draw_no_curve() {
        // Macros show plain numbers, not Hz, dB or a ratio.
        let macros: Vec<ParameterRow> = ["1 Frequency A", "1 Gain A", "1 Filter On A", "Threshold", "Ratio"]
            .iter()
            .map(|name| ParameterRow { name: name.to_string(), value: 64., min: 0., max: 127., text: Some("64".into()) })
            .collect();
        assert_eq!(eq_bands(&macros), None);
        assert_eq!(dynamics(&macros), None);
        // Live 11 calls a band's Q its Resonance.
        let live_11: Vec<ParameterRow> =
            eq_eight_after().into_iter().map(|row| ParameterRow { name: row.name.replace(" Q A", " Resonance A"), ..row }).collect();
        assert!((eq_bands(&live_11).unwrap()[0].q - 1.34).abs() < 1e-9);
        assert!(curve_parameter("2 Resonance A"));
    }

    #[test]
    fn a_send_set_alone_keeps_the_others_and_asked_levels_never_take_old_words() {
        let preview = object(json!({"prior":{"volume":0.85,"sends":[0.1,0.2,0.3,0.4]},"proposed":{"volume":0.6,"sends":[0.5]},
            "priorDisplay":{"volume":"0.0 dB","sends":["-30 dB","-24 dB","-18 dB","-12 dB"]}}));
        // Applied: Live's words for what changed.
        let applied = object(json!({"display":{"volume":"-8.0 dB","sends":["-6.0 dB"]}}));
        let ChangePicture::Mix { after, .. } = picture(kind("set_mixer"), &preview, &object(json!({})), Some(&applied)).unwrap() else {
            panic!()
        };
        assert_eq!(after.sends.iter().map(|send| send.level).collect::<Vec<_>>(), [0.5, 0.2, 0.3, 0.4]);
        assert_eq!(
            after.sends.iter().map(|send| send.text.clone().unwrap_or_default()).collect::<Vec<_>>(),
            ["-6.0 dB", "-24 dB", "-18 dB", "-12 dB"]
        );
        assert_eq!(after.volume_text.as_deref(), Some("-8.0 dB"));
        // Unconfirmed: the asked level and send have their numbers only.
        let ChangePicture::Mix { after, .. } = picture(kind("set_mixer"), &preview, &object(json!({})), None).unwrap() else { panic!() };
        assert_eq!(after.volume_text, None);
        assert_eq!(after.sends[0].text, None);
        assert_eq!(after.sends[1].text.as_deref(), Some("-24 dB"));
    }

    #[test]
    fn routing_draws_the_route_that_moved_with_arm_and_monitoring() {
        let preview = object(json!({"trackRef":"1:track:0",
            "prior":{"outputType":"Main","outputSubRouting":"","arm":false,"monitoring":1},
            "proposed":{"outputType":"Drum Bus","arm":true,"monitoring":"in"}}));
        let routed = super::picture(kind("set_routing"), &preview, &object(json!({})), None, Some("Kick")).unwrap();
        let ChangePicture::Routes { routes } = routed else { panic!() };
        let [route] = routes.as_slice() else { panic!("{routes:?}") };
        assert_eq!((route.from.as_str(), route.to.as_str()), ("Kick", "Drum Bus"), "from the track to where it goes now");
        let said: Vec<(&str, Option<&str>, Option<&str>)> =
            route.values.iter().map(|v| (v.name.as_str(), v.from_text.as_deref(), v.to_text.as_deref())).collect();
        assert_eq!(
            said,
            [("Output", Some("Main"), Some("Drum Bus")), ("Arm", Some("off"), Some("on")), ("Monitoring", Some("Auto"), Some("In"))]
        );
        // An input comes into the track; arming alone is values.
        let preview = object(json!({"prior":{"inputType":"Ext. In","inputSubRouting":"1"},"proposed":{"inputType":"Resampling"}}));
        let routed = super::picture(kind("set_routing"), &preview, &object(json!({})), None, Some("Rec")).unwrap();
        let ChangePicture::Routes { routes } = routed else { panic!() };
        assert_eq!((routes[0].from.as_str(), routes[0].to.as_str()), ("Resampling", "Rec"));
        assert_eq!(routes[0].values[0].from_text.as_deref(), Some("Ext. In (1)"));
        let armed = object(json!({"prior":{"arm":false},"proposed":{"arm":true}}));
        assert!(matches!(picture(kind("set_routing"), &armed, &object(json!({})), None), Some(ChangePicture::Values { .. })));
    }

    #[test]
    fn a_sidechain_is_a_route_into_the_devices_track() {
        let preview = object(json!({"action":"sidechain","prior":{"routingType":"No Input"}}));
        let input = object(json!({"deviceRef":"d","action":"sidechain","routingType":"Kick","routingChannel":"Post FX"}));
        let ChangePicture::Routes { routes } = super::picture(kind("set_sidechain"), &preview, &input, None, Some("Bass")).unwrap() else {
            panic!()
        };
        assert_eq!((routes[0].from.as_str(), routes[0].to.as_str()), ("Kick (Post FX)", "Bass"));
        assert_eq!((routes[0].values[0].name.as_str(), routes[0].values[0].from_text.as_deref()), ("Sidechain", Some("No Input")));
    }

    #[test]
    fn the_transport_draws_each_setting_before_and_after() {
        let preview = object(json!({"prior":{"position":0,"loop":{"enabled":false,"start":0,"length":16},"metronome":false},
            "proposed":{"loopEnabled":true,"loopStart":16,"loopLength":16,"metronome":true}}));
        let ChangePicture::Values { values } = picture(kind("set_transport"), &preview, &object(json!({})), None).unwrap() else {
            panic!()
        };
        let said: Vec<(&str, Option<&str>, Option<&str>)> =
            values.iter().map(|v| (v.name.as_str(), v.from_text.as_deref(), v.to_text.as_deref())).collect();
        assert_eq!(said, [("Loop", Some("off"), Some("bar 5 for 4 bars")), ("Metronome", Some("off"), Some("on"))]);
        let off = object(json!({"prior":{"loop":{"enabled":true,"start":0,"length":16}},"proposed":{"loopEnabled":false}}));
        let ChangePicture::Values { values } = picture(kind("set_transport"), &off, &object(json!({})), None).unwrap() else { panic!() };
        assert_eq!((values[0].from_text.as_deref(), values[0].to_text.as_deref()), (Some("bar 1 for 4 bars"), Some("off")));
    }

    #[test]
    fn locators_are_drawn_among_the_ones_there_and_a_deleted_one_where_it_was() {
        let preview = object(json!({"prior":[{"name":"Intro","position":0}],
            "proposed":[{"name":"Drop","position":64},{"name":"Outro","position":128}]}));
        let made = picture(kind("set_locators"), &preview, &object(json!({})), None).unwrap();
        let ChangePicture::Marks { before, after, values } = made else { panic!() };
        assert_eq!((before.len(), after.len()), (1, 3));
        assert_eq!(values.iter().map(|v| v.to_text.clone().unwrap()).collect::<Vec<_>>(), ["bar 17", "bar 33"]);
        let gone = object(json!({"locator":{"name":"Drop","position":64}}));
        let ChangePicture::Marks { before, after, values } = picture(kind("delete_locator"), &gone, &object(json!({})), None).unwrap()
        else {
            panic!()
        };
        assert_eq!((before.len(), after.len(), values[0].from_text.as_deref()), (1, 0, Some("bar 17")));
    }

    #[test]
    fn warp_markers_draw_as_live_answered_or_as_they_will_be() {
        let markers = json!([{"beatTime":0,"sampleTime":0},{"beatTime":4,"sampleTime":2},{"beatTime":8,"sampleTime":4}]);
        let preview = object(json!({"action":"move","markers":markers}));
        let moved = object(json!({"clipRef":"c","action":"move","beatTime":4,"distance":0.5}));
        let ChangePicture::Warp { before, after, values } = picture(kind("set_warp_markers"), &preview, &moved, None).unwrap() else {
            panic!()
        };
        assert_eq!(before[1], [4., 2.]);
        assert_eq!(after[1], [4.5, 2.], "the same moment of the sample, later in the clip");
        assert_eq!((values[0].from_text.as_deref(), values[0].to_text.as_deref()), (Some("bar 2"), Some("bar 2 beat 1.5")));
        // A new marker holds the moment the clip plays there now, on the line through its neighbours.
        let preview = object(json!({"action":"add","markers":markers}));
        let added = object(json!({"clipRef":"c","action":"add","beatTime":6}));
        let ChangePicture::Warp { after, .. } = picture(kind("set_warp_markers"), &preview, &added, None).unwrap() else { panic!() };
        assert_eq!(after, vec![[0., 0.], [4., 2.], [6., 3.], [8., 4.]]);
        // Once applied, as Live has them.
        let applied = object(json!({"markers":[{"beatTime":0,"sampleTime":0},{"beatTime":6,"sampleTime":3.1}]}));
        let ChangePicture::Warp { after, .. } = picture(kind("set_warp_markers"), &preview, &added, Some(&applied)).unwrap() else {
            panic!()
        };
        assert_eq!(after, vec![[0., 0.], [6., 3.1]]);
    }

    #[test]
    fn a_lane_is_named_as_the_preview_read_it_and_drawn_as_live_answered() {
        let preview = object(json!({"action":"insert","current":{"exists":true,"points":[{"time":0,"value":0}]},
            "parameter":{"name":"Filter Freq","min":20,"max":135}}));
        let input = object(json!({"action":"insert","points":[{"time":1,"value":60},{"time":3,"value":100}]}));
        let applied =
            object(json!({"envelope":{"exists":true,"points":[{"time":0,"value":0},{"time":1,"value":60.2},{"time":3,"value":100}]}}));
        let drawn = picture(kind("set_automation"), &preview, &input, Some(&applied)).unwrap();
        let ChangePicture::Automation { parameter, min, max, after, .. } = drawn else { panic!() };
        assert_eq!((parameter.as_str(), min, max), ("Filter Freq", 20., 135.));
        assert_eq!(after, vec![[0., 0.], [1., 60.2], [3., 100.]], "Live's points, not the ones asked for");
        // A lane Live took away is empty after.
        let gone = object(json!({"envelope":{"exists":false}}));
        let removed = picture(kind("set_automation"), &preview, &object(json!({"action":"delete-envelope"})), Some(&gone)).unwrap();
        let ChangePicture::Automation { after, .. } = removed else { panic!() };
        assert!(after.is_empty());
    }

    #[test]
    fn new_tracks_are_lit_among_the_sets_and_deleted_ones_marked() {
        let tracks = vec![("Drums".to_string(), Some("#ff5a00".to_string())), ("Bass".to_string(), None), ("Keys".to_string(), None)];
        let made = track_list(&tracks, &[(1, "Pad".into()), (3, "Lead".into())], &[]).unwrap();
        let ChangePicture::Tracks { tracks: list, .. } = made else { panic!() };
        let names: Vec<(&str, Option<TrackMark>)> = list.iter().map(|t| (t.name.as_str(), t.mark)).collect();
        assert_eq!(
            names,
            [("Drums", None), ("Pad", Some(TrackMark::Made)), ("Bass", None), ("Lead", Some(TrackMark::Made)), ("Keys", None)]
        );
        assert_eq!(list[0].color.as_deref(), Some("#ff5a00"));
        let ChangePicture::Tracks { tracks: list, .. } = track_list(&tracks, &[], &[1]).unwrap() else { panic!() };
        assert_eq!(list[1].mark, Some(TrackMark::Removed));
        assert_eq!(track_list(&tracks, &[], &[]), None);
    }
}
