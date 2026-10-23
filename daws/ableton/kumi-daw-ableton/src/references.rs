//! Live's refs: how its bridge spells them (`<epoch>:<kind>:<places>`, positions that a restructure moves), the place
//! its registry reads one again at, its discovery parents and the fields its changes take refs in. The book of the
//! turn's refs is core's (core/refs.rs).
use super::{
    changes::REFERENCE_FIELDS,
    context::{payload, PARENTS},
    mutations::track_index_of,
};
pub use crate::core::refs::{Book, KeptRefs, KnownParameter, Moved, RefScheme, ReferenceError, Shift};
use crate::{
    core::contracts::{JsonObject, ToolResult},
    mcp::types::CallToolResult,
};
use kumi_common::js::{json::stringify, string::utf16_len};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

/// The turn's refs, Live's.
pub type References = Book<LiveRefs>;

/// Live's refs, as its bridge spells them.
#[derive(Default)]
pub struct LiveRefs;

impl RefScheme for LiveRefs {
    fn positional(&self) -> bool {
        true
    }
    fn kind_of<'a>(&self, reference: &'a str) -> Option<&'a str> {
        static LIVE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+:([a-z][a-z_]{0,31}):").unwrap());
        LIVE_REF.captures(reference).and_then(|captures| captures.get(1)).map(|kind| kind.as_str())
    }
    fn moved(&self, shift: &Shift, reference: &str) -> Moved {
        live_moved(shift, reference, 0)
    }
    fn place_of(&self, reference: &str) -> String {
        place_of(reference)
    }
    fn refs_in<'a>(&self, text: &'a str) -> Vec<&'a str> {
        static REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]+:[a-z_]+:[0-9a-z_:-]+").unwrap());
        REF.find_iter(text).map(|found| found.as_str().trim_end_matches(':')).collect()
    }
    fn reference_fields(&self) -> &[String] {
        &REFERENCE_FIELDS
    }
    fn parents(&self, kind: &str) -> Option<Vec<String>> {
        PARENTS.get(kind).and_then(Value::as_array).map(|kinds| kinds.iter().filter_map(Value::as_str).map(str::to_owned).collect())
    }
    fn parent_hint(&self, kind: &str, parent: &str) -> &'static str {
        if kind == "session-clip" && parent == "track" {
            ": discover the track's clip-slots, each gives its clipRef"
        } else {
            ""
        }
    }
    fn row_refs(&self, kind: &str, row: &JsonObject) -> Vec<(String, String, bool)> {
        let mut found = vec![];
        if kind == "clip-slot" {
            if let Some(clip) = row.get("clipRef").and_then(Value::as_str).filter(|s| utf16_len(s) <= 256) {
                found.push((clip.to_owned(), "session-clip".to_owned(), true));
            }
        }
        if kind == "device" {
            for chain in row.get("chainList").and_then(Value::as_array).into_iter().flatten() {
                if let Some(reference) = chain.get("ref").and_then(Value::as_str).filter(|s| utf16_len(s) <= 256) {
                    found.push((reference.to_owned(), "chain".to_owned(), false));
                }
            }
        }
        found
    }
    fn track_of(&self, reference: &str) -> Option<String> {
        let (Some(epoch), Some(index)) = (reference.split(':').next().filter(|e| e.parse::<u64>().is_ok()), track_index_of(reference))
        else {
            return None;
        };
        Some(format!("{epoch}:track:{}", kumi_common::js::number::to_string(index)))
    }
}

/// Whether a move or a delete of a device on one of `tracks` retires this ref: Live's registry keeps places, and every
/// device, chain, parameter and drum pad there may have moved (a mixer's haven't).
pub fn retired_by_shift(reference: &str, tracks: &[f64]) -> bool {
    static DEVICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":(?:device|parameter|chain|drum_pad):").unwrap());
    DEVICE.is_match(reference) && !reference.contains(":mixer:") && track_index_of(reference).is_some_and(|index| tracks.contains(&index))
}

impl Shift {
    /// Where a Live ref points after the change: the same, another place, or nowhere (what it named was deleted).
    pub fn reference(&self, reference: &str) -> Moved {
        live_moved(self, reference, 0)
    }
}
/// Where a Live ref points after a restructure: refs are positions, a track's index counting the regular and group
/// tracks, then the returns, then Main, so a track added or deleted moves everything after it.
fn live_moved(shift: &Shift, reference: &str, depth: usize) -> Moved {
    let mut parts: Vec<String> = reference.split(':').map(str::to_owned).collect();
    if parts.len() < 3 || depth > 8 {
        return Moved::Same;
    }
    // A ref whose path is another ref (a parameter's device, a chain's rack): that one decides.
    if parts.len() >= 4 && parts[2] == parts[0] && !parts[3].is_empty() && !parts[3].bytes().all(|b| b.is_ascii_digit()) {
        return match live_moved(shift, &parts[2..].join(":"), depth + 1) {
            Moved::To(inner) => Moved::To(format!("{}:{}:{inner}", parts[0], parts[1])),
            other => other,
        };
    }
    // A track's selected-device ref (device:view:N) isn't read again by place on the Remote Script's side, so it
    // can't follow its track: once the track moves, it's retired.
    if parts[1] == "device" && parts[2] == "view" {
        return match parts.get(3).and_then(|part| part.parse::<usize>().ok()).map(|index| (index, shift.track(index))) {
            Some((index, Some(now))) if now == index => Moved::Same,
            Some(_) => Moved::Gone,
            None => Moved::Same,
        };
    }
    // Which parts hold a track's and a scene's index.
    let (track, scene) = match parts[1].as_str() {
        "track" | "device" | "chain" | "drum_pad" | "take_lane" | "take_lane_clip" | "routing_choice" | "mixer" => (Some(2), None),
        "clip" | "clip_slot" => (Some(2), Some(3)),
        // One part is a Song-level Arrangement clip, on no track.
        "arrangement_clip" if parts.len() == 4 => (Some(2), None),
        "scene" => (None, Some(2)),
        "parameter" if parts[2] == "mixer" => {
            if shift.sends && parts.get(4).is_some_and(|part| part == "sends") {
                return Moved::Gone;
            }
            (Some(3), None)
        }
        _ => (None, None),
    };
    let mut changed = false;
    for (at, place) in [(track, Shift::track as fn(&Shift, usize) -> Option<usize>), (scene, Shift::scene)] {
        let Some(at) = at else { continue };
        let Some(index) = parts.get(at).and_then(|part| part.parse::<usize>().ok()) else { continue };
        match place(shift, index) {
            None => return Moved::Gone,
            Some(now) if now != index => {
                changed = true;
                parts[at] = now.to_string();
            }
            Some(_) => {}
        }
    }
    if changed {
        Moved::To(parts.join(":"))
    } else {
        Moved::Same
    }
}

/// The track a ref is on, as the Remote Script's `_ref_track_index` reads it: refs are positions (clip:3:5,
/// device:3:0:2, parameter:mixer:3:volume), and a ref whose path is another ref (a parameter's device, a return
/// chain's rack) is on that one's track. None for one on no track (a scene, the Set, a track's view, a Song-level
/// Arrangement clip).
fn track_of(reference: &str) -> Option<usize> {
    let parts: Vec<&str> = reference.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let (kind, path) = (parts[1], &parts[2..]);
    let index = |part: &str| part.bytes().all(|b| b.is_ascii_digit()).then(|| part.parse::<usize>().ok()).flatten();
    if path.len() >= 3 && path[0] == parts[0] && index(path[1]).is_none() {
        return track_of(&path.join(":"));
    }
    match kind {
        "parameter" if path.len() >= 2 && path[0] == "mixer" => index(path[1]),
        "arrangement_clip" if path.len() == 2 => index(path[0]),
        "track" | "clip" | "clip_slot" | "device" | "chain" | "drum_pad" | "take_lane" | "take_lane_clip" | "routing_choice" => {
            index(path[0])
        }
        _ => None,
    }
}
/// The place Live's registry keeps a ref's object at, as the Remote Script's `_refresh` reads it again: the track
/// (`E:track:N`, its whole row) for what's on one; a track's Arrangement clips, which its row doesn't list, together
/// (`E:arrangement_clip:N:0` reads them all); anything else (a scene) by itself.
fn place_of(reference: &str) -> String {
    let epoch = reference.split(':').next().unwrap_or_default();
    match track_of(reference) {
        Some(track) if reference.split(':').nth(1) == Some("arrangement_clip") => format!("{epoch}:arrangement_clip:{track}:0"),
        Some(track) => format!("{epoch}:track:{track}"),
        None => reference.to_owned(),
    }
}
impl Book<LiveRefs> {
    /// What a move or a delete of a device on `track` retires (`retired_by_shift`), with its short names: kept to put
    /// back once the track's devices are where they were again.
    pub fn keep(&self, track: &str) -> KeptRefs {
        let long = self.lengthen(&json!({"ref": track}));
        let Some(index) = long["ref"].as_str().and_then(track_index_of) else { return KeptRefs::default() };
        let on = |reference: &String| retired_by_shift(reference, &[index]);
        KeptRefs {
            refs: self.refs.iter().filter(|(reference, _)| on(reference)).map(|(a, b)| (a.clone(), b.clone())).collect(),
            names: self.short.iter().filter(|(reference, _)| on(reference)).map(|(a, b)| (a.clone(), b.clone())).collect(),
        }
    }
    pub fn encode(&mut self, result: &CallToolResult, epoch: f64, slim: bool, observed_at: &str, generation: &str) -> ToolResult {
        if result.is_error == Some(true) {
            return ToolResult::error(stringify(&serde_json::to_value(result).unwrap()));
        }
        let mut live = payload(result).map(Value::Object).unwrap_or_else(|_| serde_json::to_value(result).unwrap());
        if slim {
            if let Some(row) = live.as_object() {
                live = Value::Object(slim_mixers(row));
            }
        }
        let text = stringify(
            &json!({"live":self.shorten(&live),"observation":{"observedAt":observed_at,"connectionGeneration":generation,"epoch":epoch,"coverage":"Bounded read; preserve truncated/nextCursor markers. Traversal completeness is not established."}}),
        );
        if text.len() > 64 * 1024 {
            ToolResult::error("Result too large; narrow fields/parent/page.")
        } else {
            ToolResult::text(text)
        }
    }
}
pub fn slim_mixers(content: &JsonObject) -> JsonObject {
    let Some(items) = content.get("items").and_then(Value::as_array) else { return content.clone() };
    if !items.iter().any(|v| v.as_object().is_some_and(|r| r.contains_key("mixer"))) {
        return content.clone();
    }
    let keep = ["volume", "pan", "mute", "solo", "cueVolume", "sends", "volumeDisplay", "panDisplay", "cueVolumeDisplay", "sendDisplays"];
    let items: Vec<_> = items
        .iter()
        .map(|item| {
            let Some(row) = item.as_object() else { return item.clone() };
            let Some(mixer) = row.get("mixer").filter(|v| v.is_object() || v.is_array()) else { return item.clone() };
            let mut row = row.clone();
            row.insert(
                "mixer".into(),
                Value::Object(
                    mixer
                        .as_object()
                        .map(|m| m.iter().filter(|(key, _)| keep.contains(&key.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default(),
                ),
            );
            Value::Object(row)
        })
        .collect();
    let mut result = content.clone();
    result.insert("items".into(), json!(items));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_discovered_parameter_keeps_its_name_and_range_until_its_ref_goes() {
        let mut book = References::default();
        let row = |name: &str, range: Option<(f64, f64)>| {
            let mut row = json!({"ref":"7:parameter:7:device:6:0:12","name":name}).as_object().unwrap().clone();
            if let Some((min, max)) = range {
                row.insert("min".into(), json!(min));
                row.insert("max".into(), json!(max));
            }
            row
        };
        let known = |book: &References, reference: &str| book.parameters.get(reference).cloned();
        book.register_rows("parameter", &[row("3 Gain A", Some((-15., 15.)))], &JsonObject::new(), None).unwrap();
        let gain = KnownParameter { name: "3 Gain A".into(), min: -15., max: 15. };
        assert_eq!(known(&book, "7:parameter:7:device:6:0:12"), Some(gain.clone()));
        // Read again without its range, it's still the same parameter; named otherwise, it isn't.
        book.register_rows("parameter", &[row("3 Gain A", None)], &JsonObject::new(), None).unwrap();
        assert_eq!(known(&book, "7:parameter:7:device:6:0:12"), Some(gain.clone()));
        book.register_rows("parameter", &[row("3 Q A", None)], &JsonObject::new(), None).unwrap();
        assert_eq!(known(&book, "7:parameter:7:device:6:0:12"), None);
        // It follows its track when a track before it goes, and goes with its own.
        book.register_rows("parameter", &[row("3 Gain A", Some((-15., 15.)))], &JsonObject::new(), None).unwrap();
        book.shift(&Shift { tracks_gone: vec![5], ..Default::default() });
        assert_eq!(known(&book, "7:parameter:7:device:5:0:12"), Some(gain.clone()));
        book.shift(&Shift { tracks_gone: vec![5], ..Default::default() });
        assert!(book.parameters.is_empty());
        book.register_rows("parameter", &[row("3 Gain A", Some((-15., 15.)))], &JsonObject::new(), None).unwrap();
        book.invalidate();
        assert!(book.parameters.is_empty());
    }

    #[test]
    fn a_shift_moves_every_ref_past_the_change_and_retires_what_was_deleted() {
        let to = |shift: &Shift, reference: &str| shift.reference(reference);
        // Track 5 of 8 deleted (Main at 8): what was after it moves up one, what was before stays.
        let gone = Shift { tracks_gone: vec![5], ..Default::default() };
        assert_eq!(to(&gone, "7:track:4"), Moved::Same);
        assert_eq!(to(&gone, "7:track:5"), Moved::Gone);
        assert_eq!(to(&gone, "7:track:8"), Moved::To("7:track:7".into()));
        assert_eq!(to(&gone, "7:device:6:0:1:2"), Moved::To("7:device:5:0:1:2".into()));
        assert_eq!(to(&gone, "7:clip:6:3"), Moved::To("7:clip:5:3".into()));
        assert_eq!(to(&gone, "7:clip:5:3:note:0"), Moved::Gone);
        // A ref built on another follows it: a device's parameter, a chain's volume, a rack's return chain.
        assert_eq!(to(&gone, "7:parameter:7:device:6:0:12"), Moved::To("7:parameter:7:device:5:0:12".into()));
        assert_eq!(to(&gone, "7:parameter:7:chain:6:0:1:volume"), Moved::To("7:parameter:7:chain:5:0:1:volume".into()));
        assert_eq!(to(&gone, "7:parameter:mixer:6:sends:1"), Moved::To("7:parameter:mixer:5:sends:1".into()));
        // A track's selected-device ref isn't a place Live reads again, so it's retired once its track moves.
        assert_eq!(to(&gone, "7:device:view:6"), Moved::Gone);
        assert_eq!(to(&gone, "7:device:view:4"), Moved::Same);
        // Scenes, locators and the Set aren't on a track.
        assert_eq!(to(&gone, "7:scene:6"), Moved::Same);
        assert_eq!(to(&gone, "7:locator:6"), Moved::Same);
        // Two tracks made at 2 and 4 (their places once made): the old 2 is now 3, the old 3 is now 5.
        let made = Shift { tracks_made: vec![4, 2], ..Default::default() };
        assert_eq!(to(&made, "7:track:1"), Moved::Same);
        assert_eq!(to(&made, "7:track:2"), Moved::To("7:track:3".into()));
        assert_eq!(to(&made, "7:track:3"), Moved::To("7:track:5".into()));
        // A scene made at 1 moves clips in later scenes, on every track.
        let scene = Shift { scenes_made: vec![1], ..Default::default() };
        assert_eq!(to(&scene, "7:clip_slot:4:0"), Moved::Same);
        assert_eq!(to(&scene, "7:clip_slot:4:1"), Moved::To("7:clip_slot:4:2".into()));
        assert_eq!(to(&scene, "7:scene:3"), Moved::To("7:scene:4".into()));
        // A return deleted renumbers every track's sends, so they're retired.
        let returns = Shift { tracks_gone: vec![9], sends: true, ..Default::default() };
        assert_eq!(to(&returns, "7:parameter:mixer:2:sends:0"), Moved::Gone);
        assert_eq!(to(&returns, "7:parameter:mixer:2:volume"), Moved::Same);
    }

    #[test]
    fn mains_device_refs_kept_before_a_move_there_come_back_with_their_names() {
        let mut book = References::default();
        for reference in ["7:track:6", "7:device:6:0", "7:parameter:7:device:6:0:12", "7:device:2:0", "7:parameter:mixer:6:volume"] {
            book.refs.insert(reference.into(), "x".into());
        }
        let (main, limiter, gain) =
            (book.short_ref("7:track:6"), book.short_ref("7:device:6:0"), book.short_ref("7:parameter:7:device:6:0:12"));
        // Kept by Main's short name as well as its long one.
        let kept = book.keep(&main);
        // A move and a delete on Main retire its devices' refs and their parameters', not its mixer's or another track's.
        for reference in book.refs.keys().cloned().chain(book.named_references()).collect::<Vec<_>>() {
            if retired_by_shift(&reference, &[6.]) {
                book.retire(&reference);
            }
        }
        assert_eq!(book.refs.keys().cloned().collect::<Vec<_>>(), ["7:track:6", "7:device:2:0", "7:parameter:mixer:6:volume"]);
        // Kumi's own read of Main meanwhile names its first device anew.
        let since = book.short_ref("7:device:6:0");
        assert_ne!(since, limiter);
        book.put_back(kept);
        assert!(book.refs.contains_key("7:device:6:0") && book.refs.contains_key("7:parameter:7:device:6:0:12"));
        assert_eq!(book.lengthen(&json!({"ref": gain}))["ref"], "7:parameter:7:device:6:0:12");
        assert_eq!(book.short_ref("7:device:6:0"), limiter);
        assert_eq!(book.lengthen(&json!({"ref": since}))["ref"], json!(since));
        // Nothing was kept for a track Kumi can't place.
        assert!(book.keep("nowhere").refs.is_empty());
    }

    #[test]
    fn short_names_follow_their_refs_and_a_deleted_ones_name_names_nothing() {
        let mut book = References::default();
        for index in 0..4 {
            book.refs.insert(format!("7:track:{index}"), "track".into());
        }
        let names: Vec<String> = (0..4).map(|index| book.short_ref(&format!("7:track:{index}"))).collect();
        book.shift(&Shift { tracks_gone: vec![1], ..Default::default() });
        assert_eq!(book.refs.keys().cloned().collect::<Vec<_>>(), ["7:track:0", "7:track:1", "7:track:2"]);
        assert_eq!(book.lengthen(&json!({"ref":names[2]}))["ref"], "7:track:1");
        assert_eq!(book.lengthen(&json!({"ref":names[1]}))["ref"], json!(names[1]));
        // A name keeps its object: the track now at 1 is still called by the old 2's name.
        assert_eq!(book.short_ref("7:track:1"), names[2]);
    }

    #[test]
    fn a_moved_ref_names_the_place_live_reads_again_before_python_uses_it() {
        let mut book = References::default();
        for reference in ["7:track:0", "7:device:2:0", "7:parameter:7:device:2:0:3", "7:chain:7:device:2:0:return:0"] {
            book.refs.insert(reference.into(), "x".into());
        }
        for reference in ["7:parameter:mixer:2:volume", "7:clip:2:0", "7:arrangement_clip:2:0", "7:scene:0", "7:device:view:2"] {
            book.refs.insert(reference.into(), "x".into());
        }
        book.shift(&Shift { tracks_gone: vec![1], ..Default::default() });
        // Track 0 didn't move: nothing to read again.
        assert!(book.moved_places(&["7:track:0"]).is_empty());
        // What was on track 2 is on 1 now, and Live still holds old track 1's under those strings: Python naming them
        // has track 1 read again first, once for all of them.
        let code = r#"ARGS = json.loads("[{\"ref\":\"7:parameter:7:device:1:0:3\"},{\"device\":\"7:device:1:0\"}]")"#;
        assert_eq!(book.moved_places(&[code]), ["7:track:1"]);
        // A rack's return chain and a mixer's parameter are on their track too; Arrangement clips are read apart.
        let more = "7:chain:7:device:1:0:return:0 7:parameter:mixer:1:volume 7:arrangement_clip:1:0";
        assert_eq!(book.moved_places(&[more]), ["7:track:1", "7:arrangement_clip:1:0"]);
        assert!(!book.refs.contains_key("7:device:view:1"), "a view ref is retired, not moved");
        // Track 1 read again: what's on it is current, but not its Arrangement clips, which its row doesn't list.
        book.read_again(&["7:track:1".to_owned()]);
        assert!(book.moved_places(&[code]).is_empty());
        assert_eq!(book.moved_places(&[more]), ["7:arrangement_clip:1:0"]);
        // Live registering a ref (a read, a change that made it) leaves nothing to read again there.
        book.registered("7:arrangement_clip:1:0");
        assert!(book.moved_places(&[more]).is_empty());
        // A scene made first moves every scene, and every clip, after it; what was marked moves along.
        book.shift(&Shift { scenes_made: vec![0], ..Default::default() });
        assert_eq!(book.moved_places(&["7:scene:1", "7:clip:1:1"]), ["7:scene:1", "7:track:1"]);
        assert!(book.moved_places(&["7:clip:1:0"]).is_empty(), "nothing Kumi knows is at scene 0 now");
    }
}
