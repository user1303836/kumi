//! The look a whole turn plans from (`observe` with scope `project`, worklist item 3): the project as the DAW describes
//! it, what decides a request said first, then the steps a change may use, one line each, and the tracks last, folded
//! round the one in focus to what's left of the text's room. A model plans an ordinary change from this one look, with
//! no read of a step's schema; and what's cut to fit is the far end of the track list, never what's selected, what Kumi
//! changed or what changed since the last look.

use kumi_agent::core::fold::track_line;
use serde_json::{json, Map, Value};

/// The context's fields that decide a request, said first and in this order: what "this" means and what's selected,
/// what changed (a reconnection, a crash's restore, Kumi's own changes, what changed since the last look), what the
/// producer pointed at, then the project and its transport.
const FIRST: [&str; 13] = [
    "this",
    "selectedInLive",
    "selection",
    "reconnected",
    "restoredAfterCrash",
    "kumiChanges",
    "sinceLastTime",
    "pinned",
    "references",
    "set",
    "project",
    "song",
    "transport",
];

/// The track list and what's said of its folding, said last: the far end of it is what's cut to fit.
const LAST: [&str; 4] = ["folded", "moreTracks", "moreDevices", "tracks"];

/// The share of the text's room the steps may take (the rest is the project's).
const STEPS_SHARE: (usize, usize) = (2, 5);

/// What a folded list says, when the DAW's look didn't fold it itself.
const FOLDED: &str = "Some tracks are folded to fit; read (action data) with the DAW's reader, kind track, reads them whole.";

/// A step a change may use, as the look says it: its line (what it does and its input's fields), its fields alone, and
/// its name.
pub(super) struct Step {
    pub name: &'static str,
    pub line: String,
    pub fields: Vec<String>,
}

/// The project's `context` (the DAW's description of it, as JSON) as one look says it within `most` characters: what
/// decides a request first, then the `steps` (each as whole as its share of the room holds) and the `readers`, then the
/// rest, and the tracks last, folded round the one in focus. A context that isn't a JSON object is said as it is.
pub(super) fn whole_turn(context: &str, steps: &[Step], readers: &[String], most: usize) -> String {
    let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(context) else {
        return context.to_string();
    };
    let focused = focused_track(&fields);
    if let Some(this) = this_track(&fields, focused.as_deref()) {
        fields.insert("this".into(), json!(this));
    }
    let mut said = Map::new();
    for key in FIRST {
        if let Some(value) = fields.shift_remove(key) {
            said.insert(key.into(), value);
        }
    }
    if !steps.is_empty() {
        said.insert("steps".into(), steps_said(steps, most * STEPS_SHARE.0 / STEPS_SHARE.1));
    }
    if !readers.is_empty() {
        said.insert("readers".into(), json!(readers));
    }
    let last: Vec<(String, Value)> =
        LAST.iter().filter_map(|key| fields.shift_remove(*key).map(|value| (key.to_string(), value))).collect();
    said.extend(fields);
    said.extend(last);
    fit_tracks(&mut said, focused.as_deref(), most);
    Value::Object(said).to_string()
}

/// The track in focus, by the ref its row has: Live's selected track, or an own bridge's selection's track.
fn focused_track(fields: &Map<String, Value>) -> Option<String> {
    fields
        .get("selectedInLive")
        .and_then(|selected| selected.pointer("/track/ref"))
        .or_else(|| fields.get("selection").and_then(|selection| selection.get("trackRef")))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// What "this" means, said as Live's own look says it, for a DAW whose look doesn't: the selected track by its ref and
/// name.
fn this_track(fields: &Map<String, Value>, focused: Option<&str>) -> Option<String> {
    if fields.contains_key("selectedInLive") {
        return None;
    }
    let focused = focused?;
    let name = fields
        .get("tracks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|row| row.get("ref").and_then(Value::as_str) == Some(focused))
        .and_then(|row| row.get("name").and_then(Value::as_str));
    let named = name.map(|name| format!(" (\"{}\")", super::change::one_line(name, 60).replace('"', "'"))).unwrap_or_default();
    Some(format!("\"This track\" or \"here\" in the producer's words means {focused}{named}, unless they pointed at something in Kumi."))
}

/// The steps as their share of the room holds them, in their order (the DAW's allowlist puts the usual ones first): each
/// one's whole line, else its name and fields, else its name, with room always kept for the names of those after it, so
/// every step is listed.
fn steps_said(steps: &[Step], room: usize) -> Value {
    // What a string costs in the list: itself, its quotes and the comma after it.
    let cost = |text: &str| serde_json::to_string(text).map_or(text.len() + 2, |quoted| quoted.len()) + 1;
    let fielded = |step: &Step| {
        if step.fields.is_empty() {
            step.name.to_string()
        } else {
            format!("{} ({})", step.name, step.fields.join(", "))
        }
    };
    let mut names_after: Vec<usize> = steps
        .iter()
        .rev()
        .scan(0, |sum, step| {
            *sum += cost(step.name);
            Some(*sum)
        })
        .collect();
    names_after.reverse();
    names_after.push(0);
    let mut used = 2;
    let mut said = Vec::with_capacity(steps.len());
    for (at, step) in steps.iter().enumerate() {
        let reserve = names_after[at + 1];
        let shown = [step.line.clone(), fielded(step)]
            .into_iter()
            .find(|text| used + cost(text) + reserve <= room)
            .unwrap_or_else(|| step.name.to_string());
        used += cost(&shown);
        said.push(shown);
    }
    json!(said)
}

/// The track list folded to the room the rest leaves of `most`: the track in focus kept as it is, the others one line
/// each with their devices named, then without them, then the far end left out, said how many and how to read them.
/// The list stays last, and the look fits `most` whenever the rest of it does.
fn fit_tracks(said: &mut Map<String, Value>, focused: Option<&str>, most: usize) {
    let Some(Value::Array(rows)) = said.get("tracks").cloned() else { return };
    let size = |said: &Map<String, Value>| Value::Object(said.clone()).to_string().chars().count();
    if size(said) <= most {
        return;
    }
    let in_focus = |row: &Value| focused.is_some() && row.get("ref").and_then(Value::as_str) == focused;
    let lined = |names: bool| -> Vec<Value> {
        rows.iter()
            .map(|row| match row {
                Value::Object(track) if !in_focus(row) => Value::String(track_line(track, names)),
                _ => row.clone(),
            })
            .collect()
    };
    let theirs = said.get("moreTracks").and_then(Value::as_str).map(str::to_owned);
    // The list as it'll be said: its notes, then the rows, last.
    let put = |said: &mut Map<String, Value>, kept: Vec<Value>, left: usize| {
        said.shift_remove("tracks");
        said.entry("folded").or_insert_with(|| json!(FOLDED));
        if left > 0 {
            let here = format!(
                "{left} more of the project's tracks aren't listed here; read (action data) with the DAW's reader, kind track, pages through them"
            );
            let more = match &theirs {
                Some(theirs) => format!("{theirs}. {here}"),
                None => here,
            };
            said.insert("moreTracks".into(), json!(more));
        }
        said.insert("tracks".into(), Value::Array(kept));
    };
    for names in [true, false] {
        put(said, lined(names), 0);
        if size(said) <= most {
            return;
        }
    }
    // The far end goes, a track at a time, never the one in focus: by what each row costs, with room for the note that
    // says how many went; then once more by the whole look's own size, for what that note's count costs.
    let mut kept = lined(false);
    let mut left = 0;
    let mut over = size(said).saturating_sub(most);
    while left == 0 || size(said) > most {
        put(said, kept.clone(), left.max(1));
        over = over.max(size(said).saturating_sub(most));
        while over > 0 {
            let Some(at) = kept.iter().rposition(|row| !in_focus(row)) else { break };
            over = over.saturating_sub(kept.remove(at).to_string().chars().count() + 1);
            left += 1;
        }
        put(said, kept.clone(), left);
        if over > 0 || !kept.iter().any(|row| !in_focus(row)) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(name: &'static str) -> Step {
        Step {
            name,
            line: format!("{name}: change something about a track in the project, as the producer asks (trackRef, value?)"),
            fields: vec!["trackRef".into(), "value?".into()],
        }
    }

    fn track(n: usize) -> Value {
        json!({"ref": format!("track:{n}"), "name": format!("Track {n}"), "type": "audio",
            "devices": [{"name": "EQ Eight"}, {"name": "Compressor"}, {"name": "Saturator"}], "volume": -6.0, "pan": 0.0})
    }

    /// A project too big for the room keeps, whole and first, what decides a request: what "this" means, what Kumi
    /// changed and what changed since the last look; then the steps; and the track in focus as it is, whatever is cut.
    /// The look fits its room.
    #[test]
    fn what_decides_a_request_comes_first_and_the_far_tracks_are_what_a_small_room_cuts() {
        let tracks: Vec<Value> = (1..=200).map(track).collect();
        let context = json!({
            "observedAt": "now", "daw": "REAPER", "project": {"name": "Song"}, "tracks": tracks,
            "selection": {"trackRef": "track:150"}, "kumiChanges": [{"title": "Bass −3 dB"}],
            "sinceLastTime": {"lines": ["Track 4 was added"]},
        })
        .to_string();
        let steps: Vec<Step> = ["track_volume", "track_pan", "fx_add"].into_iter().map(step).collect();
        let said = whole_turn(&context, &steps, &["reaper_read".into()], 6000);
        assert!(said.chars().count() <= 6000, "{}", said.chars().count());
        let said: Value = serde_json::from_str(&said).unwrap();
        let keys: Vec<&str> = said.as_object().unwrap().keys().map(String::as_str).collect();
        let at = |key: &str| keys.iter().position(|k| *k == key).unwrap_or_else(|| panic!("{key} in {keys:?}"));
        assert!(at("this") < at("selection") && at("selection") < at("kumiChanges") && at("kumiChanges") < at("sinceLastTime"));
        assert!(at("sinceLastTime") < at("steps") && at("steps") < at("tracks") && *keys.last().unwrap() == "tracks", "{keys:?}");
        assert!(said["this"].as_str().unwrap().contains("track:150 (\"Track 150\")"), "{}", said["this"]);
        assert_eq!(said["steps"][0], json!(steps[0].line), "whole lines while they fit");
        let rows = said["tracks"].as_array().unwrap();
        assert!(rows.iter().any(|row| row == &track(150)), "the track in focus as it is");
        let more = said["moreTracks"].as_str().unwrap();
        assert!(rows.len() < 200 && more.starts_with(&format!("{} more of the project's tracks", 200 - rows.len())), "{more}");
        assert!(said["folded"].is_string());
    }

    /// A project that fits is said whole: nothing folded, and the steps their whole lines.
    #[test]
    fn a_project_that_fits_is_said_whole_with_its_steps() {
        let context = json!({"daw": "REAPER", "tracks": [track(1), track(2)], "selection": {"trackRef": "track:2"}}).to_string();
        let said: Value = serde_json::from_str(&whole_turn(&context, &[step("track_volume")], &[], 12000)).unwrap();
        assert_eq!(said["tracks"], json!([track(1), track(2)]));
        assert!(said.get("folded").is_none() && said.get("moreTracks").is_none());
        assert_eq!(said["steps"], json!([step("track_volume").line]));
    }

    /// Steps that don't all fit their share whole: the first ones (the usual ones) whole, the rest by their fields, then
    /// by name; every one listed, never cut mid-list.
    #[test]
    fn the_first_steps_are_said_whole_and_the_rest_shorter_never_cut() {
        let steps: Vec<Step> = (0..60).map(|_| step("track_volume")).collect();
        let said = steps_said(&steps, 3000);
        let said = said.as_array().unwrap();
        assert_eq!((said.len(), said[0].clone()), (60, json!(steps[0].line)));
        assert!(said.iter().any(|line| line == "track_volume (trackRef, value?)"), "then by their fields");
        assert_eq!(said[59], json!("track_volume"), "the last by name");
        assert!(json!(said).to_string().len() <= 3000);
        // A room smaller than the names alone: every step by name, all of them still listed.
        let named = steps_said(&steps, 600);
        assert_eq!((named.as_array().unwrap().len(), named[0].clone()), (60, json!("track_volume")), "names when that's all that fits");
    }

    /// Live's look says what "this" means itself: nothing is added; and a context that isn't JSON is said as it is.
    #[test]
    fn lives_own_words_for_this_stand_and_text_stays_text() {
        let context = json!({"selectedInLive": {"track": {"ref": "track:1", "name": "Drums"}, "note": "\"This track\"…"},
            "tracks": [track(1)]})
        .to_string();
        let said: Value = serde_json::from_str(&whole_turn(&context, &[], &[], 12000)).unwrap();
        assert!(said.get("this").is_none() && said["selectedInLive"]["note"].is_string());
        assert_eq!(whole_turn("Live couldn't be read", &[], &[], 12000), "Live couldn't be read");
    }
}
