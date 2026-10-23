//! What hides a Max for Live device's own face (#179). Max draws a box above the boxes listed after it in the
//! same layer, and background boxes (`background: 1`) below every other. A face styled by editing the device's
//! file put a full-size opaque backdrop first among its decorations, and Live showed a plain black face; the
//! model, which can't see Live, said it looked like a rave flyer. This reads a device's patcher, with no Max and
//! no Live, and says what its panels hide.
use std::path::Path;

use serde_json::Value;

use super::amxd::decode_amxd;

/// Fills at least this opaque hide what's behind them.
const OPAQUE: f64 = 0.95;
/// A box counts as hidden when a panel covers at least this much of it.
const COVERED: f64 = 0.6;
/// The objects named in a note, at most.
const NAMED: usize = 5;
/// The device files read, at most (a device with samples frozen in can be big; a patcher isn't).
const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn of(value: &Value) -> Option<Rect> {
        let numbers: Vec<f64> = value.as_array()?.iter().filter_map(Value::as_f64).collect();
        match numbers[..] {
            [x, y, w, h] if w > 0. && h > 0. && [x, y, w, h].iter().all(|n| n.is_finite()) => Some(Rect { x, y, w, h }),
            _ => None,
        }
    }
    /// How much of `other` this covers, from 0 to 1.
    fn covers(&self, other: &Rect) -> f64 {
        let width = (self.x + self.w).min(other.x + other.w) - self.x.max(other.x);
        let height = (self.y + self.h).min(other.y + other.h) - self.y.max(other.y);
        if width <= 0. || height <= 0. {
            0.
        } else {
            width * height / (other.w * other.h)
        }
    }
}

/// A colour's alpha (Max colours are [r, g, b, a] from 0 to 1); a colour without one is opaque.
fn alpha(colour: &Value) -> f64 {
    colour.as_array().and_then(|c| c.get(3)).and_then(Value::as_f64).unwrap_or(1.)
}

/// How opaque a panel's fill is: its bgfillcolor (a colour, or the less opaque end of a gradient), else its
/// bgcolor, else Max's default panel fill, which is opaque.
fn fill(panel: &Value) -> f64 {
    match panel.get("bgfillcolor") {
        Some(fill) if fill["type"] == "gradient" => alpha(&fill["color1"]).min(alpha(&fill["color2"])),
        Some(fill) if fill.get("color").is_some() => alpha(&fill["color"]),
        _ => panel.get("bgcolor").map(alpha).unwrap_or(1.),
    }
}

/// A box as a producer would know it: its scripting name, a comment's words, else its id.
fn name(item: &Value) -> String {
    let words = |text: &str| {
        let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.chars().count() > 30 {
            format!("{}…", text.chars().take(29).collect::<String>())
        } else {
            text
        }
    };
    item["varname"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| item["text"].as_str().filter(|s| !s.trim().is_empty()).map(words))
        .or_else(|| item["id"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "a box".into())
}

/// What the device's opaque panels hide on its face, a line each: the panel, and the presentation objects listed
/// after it in its layer that it covers. Empty when nothing is hidden.
pub fn hidden_by_panels(patcher: &Value) -> Vec<String> {
    let boxes: Vec<&Value> = patcher["patcher"]["boxes"].as_array().into_iter().flatten().map(|entry| &entry["box"]).collect();
    let shown = |item: &Value| item["presentation"].as_f64() == Some(1.);
    let layer = |item: &Value| item["background"].as_f64() == Some(1.);
    let mut lines = vec![];
    for (at, panel) in boxes.iter().enumerate() {
        if panel["maxclass"] != "panel" || !shown(panel) || fill(panel) < OPAQUE {
            continue;
        }
        let Some(area) = Rect::of(&panel["presentation_rect"]) else { continue };
        let hidden: Vec<String> = boxes[at + 1..]
            .iter()
            .filter(|item| shown(item) && layer(item) == layer(panel))
            .filter(|item| Rect::of(&item["presentation_rect"]).is_some_and(|rect| area.covers(&rect) >= COVERED))
            .map(|item| format!("`{}`", name(item)))
            .collect();
        if hidden.is_empty() {
            continue;
        }
        let named = hidden.iter().take(NAMED).cloned().collect::<Vec<_>>().join(", ");
        lines.push(format!(
            "`{}` ({}×{}, opaque) is drawn over {} listed after it in its layer: {named}{}",
            name(panel),
            area.w.round(),
            area.h.round(),
            if hidden.len() == 1 { "1 object".to_owned() } else { format!("{} objects", hidden.len()) },
            if hidden.len() > NAMED { ", …" } else { "" }
        ));
    }
    lines
}

/// For a device just loaded from the User Library (`user_library/…/name.amxd`, as the Browser names it): what
/// hides its face, as a note for the model, which can't see Live. None for anything else, or nothing hidden.
pub fn face_note(user_library: &Path, item_id: &str) -> Option<String> {
    let relative = item_id.strip_prefix("user_library/")?;
    // A Browser id for a second item at one path ends "#2" (#183): the file is the path's.
    let relative = relative.rsplit_once('#').filter(|(_, n)| n.chars().all(|c| c.is_ascii_digit())).map_or(relative, |(path, _)| path);
    // Only a path inside the library: no part that climbs out, and none Windows reads as more than one part
    // (`..\x`) or as a drive (`C:`).
    if !relative.to_lowercase().ends_with(".amxd")
        || relative.split('/').any(|part| part.is_empty() || part == ".." || part.contains(['\\', ':']))
    {
        return None;
    }
    let file = relative.split('/').fold(user_library.to_path_buf(), |path, part| path.join(part));
    if std::fs::metadata(&file).ok()?.len() > MAX_BYTES {
        return None;
    }
    let lines = hidden_by_panels(&decode_amxd(&std::fs::read(&file).ok()?)?.patcher);
    (!lines.is_empty()).then(|| {
        format!(
            "Its face may not show as built: {}. Max draws a box above the boxes after it in the same layer, and background boxes below every other: put a full-size backdrop last among the background boxes, and text before the panel it sits on. Tell the producer what they'll see until it's fixed.",
            lines.join("; ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::amxd::{encode_amxd, DeviceType};
    use serde_json::json;

    fn face(boxes: Vec<Value>) -> Value {
        json!({"patcher":{"boxes":boxes.into_iter().map(|item| json!({"box":item})).collect::<Vec<_>>()}})
    }
    fn panel(id: &str, rect: [f64; 4], background: bool, colour: [f64; 4]) -> Value {
        json!({"id":id,"varname":id,"maxclass":"panel","presentation":1,"presentation_rect":rect,"background":if background {1} else {0},"bgcolor":colour})
    }
    fn comment(id: &str, text: &str, rect: [f64; 4]) -> Value {
        json!({"id":id,"maxclass":"comment","text":text,"presentation":1,"presentation_rect":rect,"background":1})
    }
    fn dial(id: &str, rect: [f64; 4]) -> Value {
        json!({"id":id,"varname":id,"maxclass":"live.dial","presentation":1,"presentation_rect":rect})
    }

    #[test]
    fn a_backdrop_listed_first_hides_the_decorations_after_it_and_not_the_controls() {
        // #179's rave flyer: the full-size backdrop first among the background boxes.
        let flyer = face(vec![
            panel("rave-bg", [0., 0., 728., 169.], true, [0.1, 0.0, 0.1, 1.]),
            panel("rave-left", [0., 0., 160., 169.], true, [0.4, 0.1, 0.6, 1.]),
            comment("obj-9", "PANIC // VHS", [10., 10., 140., 30.]),
            comment("obj-10", "STAIRCASE MASSACRE", [10., 40., 140., 20.]),
            dial("cutoff", [200., 20., 44., 48.]),
        ]);
        let lines = hidden_by_panels(&flyer);
        assert_eq!(lines[0], "`rave-bg` (728×169, opaque) is drawn over 3 objects listed after it in its layer: `rave-left`, `PANIC // VHS`, `STAIRCASE MASSACRE`");
        // The side panel hides the title on it too, and nothing hides the dial (a layer above).
        assert_eq!(
            lines[1],
            "`rave-left` (160×169, opaque) is drawn over 2 objects listed after it in its layer: `PANIC // VHS`, `STAIRCASE MASSACRE`"
        );
        assert_eq!(lines.len(), 2);
        // The title before the panel it sits on, the backdrop last among the background boxes: nothing hidden.
        let fixed = face(vec![
            comment("obj-9", "PANIC // VHS", [10., 10., 140., 30.]),
            panel("rave-left", [0., 0., 160., 169.], true, [0.4, 0.1, 0.6, 1.]),
            panel("rave-bg", [0., 0., 728., 169.], true, [0.1, 0.0, 0.1, 1.]),
            dial("cutoff", [200., 20., 44., 48.]),
        ]);
        assert!(hidden_by_panels(&fixed).is_empty(), "{:?}", hidden_by_panels(&fixed));
    }

    #[test]
    fn a_see_through_panel_or_one_beside_the_rest_hides_nothing() {
        let glass =
            face(vec![panel("glass", [0., 0., 728., 169.], true, [1., 1., 1., 0.3]), comment("obj-2", "title", [10., 10., 100., 20.])]);
        assert!(hidden_by_panels(&glass).is_empty());
        let beside =
            face(vec![panel("strip", [0., 150., 728., 19.], true, [1., 0., 1., 1.]), comment("obj-2", "title", [10., 10., 100., 20.])]);
        assert!(hidden_by_panels(&beside).is_empty());
        let gradient = json!({"id":"fade","maxclass":"panel","presentation":1,"presentation_rect":[0,0,728,169],"background":1,
            "bgfillcolor":{"type":"gradient","color1":[0,0,0,1],"color2":[0,0,0,0]}});
        assert!(
            hidden_by_panels(&face(vec![gradient, comment("obj-2", "title", [10., 10., 100., 20.])])).is_empty(),
            "a gradient to clear"
        );
    }

    #[test]
    fn a_device_loaded_from_the_user_library_says_what_hides_its_face() {
        let library = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(library.path().join("Kumi")).unwrap();
        let flyer = face(vec![
            panel("rave-bg", [0., 0., 728., 169.], true, [0., 0., 0., 1.]),
            comment("obj-2", "ILLEGAL SIGNAL", [10., 10., 100., 20.]),
        ]);
        std::fs::write(library.path().join("Kumi").join("PANIC VHS.amxd"), encode_amxd(DeviceType::AudioEffect, &flyer)).unwrap();
        let note = face_note(library.path(), "user_library/Kumi/PANIC VHS.amxd").unwrap();
        assert!(note.starts_with("Its face may not show as built: `rave-bg` (728×169, opaque) is drawn over 1 object"), "{note}");
        assert_eq!(face_note(library.path(), "user_library/Kumi/PANIC VHS.amxd#2"), Some(note), "a repeat's id is the same file");
        assert_eq!(face_note(library.path(), "audio_effects/Reverb"), None);
        assert_eq!(face_note(library.path(), "user_library/Kumi/../secret.amxd"), None);
        assert_eq!(face_note(library.path(), "user_library/Kumi/..\\..\\secret.amxd"), None, "Windows' separator");
        assert_eq!(face_note(library.path(), "user_library/C:/secret.amxd"), None, "a drive");
        assert_eq!(face_note(library.path(), "user_library/Kumi/Missing.amxd"), None);
    }
}
