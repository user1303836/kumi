//! A device's Map page: a page of mapping slots, as Live's own modulators have, for any device whose patch sends
//! modulation to `mod`. Each slot maps one parameter anywhere in Live with its Map button, which shows the mapped
//! parameter's name once it's clicked. A slot has:
//! - × to unmap it;
//! - Min and Max: the part of the modulation's range the source sweeps, in percent (Max below Min turns it upside down);
//! - when the patch sends several sources (`mod.0`, `mod.1`…), the source it follows.
//!
//! Each slot drives a live.modulate~. It modulates the parameter from where its own control is, the way Live's
//! modulators do, so the producer can still turn it, and it adds nothing to Live's undo history. A Max without
//! live.modulate~ (Live 11's) gets live.remote~, which sets the parameter outright. live.map keeps each mapping with the
//! Set, and gives the slot back its parameter's name when the Set opens again. It's told not to map the device's own
//! parameters, so a slot can't modulate the device itself.
//!
//! The page is an embedded bpatcher, so the device holds all of it.

use serde_json::{json, Value};

use super::look::theme;

/// How many slots a Map page has, as Live's own modulators do.
pub const SLOTS: usize = 8;
/// The most slots (the page is as tall as the face), and the most sources.
pub const MOST_SLOTS: usize = 8;
pub const MOST_SOURCES: usize = 8;
/// A slot's row.
const ROW: f64 = 18.0;
const HEADER: f64 = 15.0;
const MAP: f64 = 96.0;
const UNMAP: f64 = 15.0;
const SOURCE: f64 = 58.0;
const AMOUNT: f64 = 40.0;

/// The page's width and height, for `sources` and `slots`.
pub fn size(sources: usize, slots: usize) -> (f64, f64) {
    let width = MAP + 2.0 + UNMAP + 2.0 + if sources > 1 { SOURCE + 2.0 } else { 0.0 } + AMOUNT + 2.0 + AMOUNT;
    (width, HEADER + slots as f64 * ROW)
}

/// The scaler's GenExpr: each slot's modulation from its source (in1…, 0 to 1), between its Min and Max (percent of the
/// modulation's range). For live.modulate~ (`modulate`) that's −1 to 1: a parameter Live modulates both ways moves
/// around where its control is, and one it modulates one way moves from its minimum up to its control. For live.remote~
/// it's 0 to 1, the parameter's own range.
pub fn scaler(sources: usize, slots: usize, modulate: bool) -> String {
    let mut code = String::from(
        "// Made by Kumi: each slot's modulation, from its source (0 to 1), between its Min and Max (percent of the modulation's range).\n",
    );
    for slot in 1..=slots {
        code.push_str(&format!("Param min{slot}(0);\nParam max{slot}(100);\n"));
        if sources > 1 {
            code.push_str(&format!("Param source{slot}(0);\n"));
        }
    }
    for slot in 1..=slots {
        let mut pick = format!("in{sources}");
        for source in (1..sources).rev() {
            pick = format!("source{slot} < {}.5 ? in{source} : ({pick})", source - 1);
        }
        let pick = if sources > 1 { pick } else { "in1".to_string() };
        let range = if modulate { "* 0.02 - 1" } else { "* 0.01" };
        code.push_str(&format!("s{slot} = clamp({pick}, 0, 1);\nout{slot} = (min{slot} + s{slot} * (max{slot} - min{slot})) {range};\n"));
    }
    code
}

fn object(id: &str, text: &str, ins: usize, outs: usize, at: [f64; 2]) -> Value {
    json!({ "id": id, "maxclass": "newobj", "text": text, "numinlets": ins, "numoutlets": outs, "outlettype": vec![""; outs],
        "patching_rect": [at[0], at[1], 20.0 + text.len() as f64 * 6.5, 22.0] })
}

fn message(id: &str, text: &str, at: [f64; 2]) -> Value {
    json!({ "id": id, "maxclass": "message", "text": text, "numinlets": 2, "numoutlets": 1, "outlettype": [""], "patching_rect": [at[0], at[1], 20.0 + text.len() as f64 * 6.5, 22.0] })
}

fn cord(from: &str, outlet: usize, to: &str, inlet: usize) -> Value {
    json!({ "patchline": { "source": [from, outlet], "destination": [to, inlet] } })
}

/// A parameter's `valueof`: shown to Live as `invisible` says (1 stored only, 2 hidden).
fn parameter(name: &str, short: &str, invisible: u32, more: Value) -> Value {
    let mut valueof = json!({ "parameter_longname": name, "parameter_shortname": short, "parameter_invisible": invisible, "parameter_initial_enable": 1 });
    if let (Value::Object(valueof), Value::Object(more)) = (&mut valueof, more) {
        valueof.extend(more);
    }
    valueof
}

/// A box in the page's presentation, with a theme's colours for its attributes.
fn shown(mut item: Value, rect: [f64; 4], colours: &[(&str, &str)], valueof: Option<Value>) -> Value {
    item["presentation"] = json!(1);
    item["presentation_rect"] = json!(rect);
    let mut saved = serde_json::Map::new();
    for (attribute, colour) in colours {
        let (value, expression) = theme(attribute, colour);
        if let (Value::Object(item), Value::Object(value)) = (&mut item, value) {
            item.extend(value);
        }
        if let Value::Object(expression) = expression {
            saved.extend(expression);
        }
    }
    if let Some(valueof) = valueof {
        item["parameter_enable"] = json!(1);
        saved.insert("valueof".into(), valueof);
    }
    if !saved.is_empty() {
        item["saved_attribute_attributes"] = Value::Object(saved);
    }
    item
}

/// The Map page's bpatcher, as box `id` with scripting name `name`, hidden until its tab shows it: an inlet for each
/// of `sources` (named for the producer), `slots` slots, and live.modulate~ where `modulate` (else live.remote~).
pub fn page(id: &str, name: &str, sources: &[String], slots: usize, modulate: bool) -> Value {
    let count = sources.len().clamp(1, MOST_SOURCES);
    let slots = slots.clamp(1, MOST_SLOTS);
    let (width, height) = size(count, slots);
    let mut boxes: Vec<Value> = Vec::new();
    let mut lines: Vec<Value> = Vec::new();
    for (at, source) in sources.iter().take(count).enumerate() {
        boxes.push(
            json!({ "id": format!("in-{}", at + 1), "maxclass": "inlet", "index": at + 1, "comment": format!("(signal 0 to 1) {source}"),
            "numinlets": 0, "numoutlets": 1, "outlettype": ["signal"], "patching_rect": [40.0 + at as f64 * 60.0, 20.0, 30.0, 30.0] }),
        );
    }
    let code = scaler(count, slots, modulate);
    boxes.push(json!({ "id": "scale", "maxclass": "newobj", "text": "gen~", "numinlets": count, "numoutlets": slots, "outlettype": vec!["signal"; slots],
        "patching_rect": [40.0, 80.0, 60.0, 22.0], "patcher": super::gen::gen_patcher(&code, count, slots) }));
    for at in 0..count {
        lines.push(cord(&format!("in-{}", at + 1), 0, "scale", at));
    }
    let comment = |id: &str, text: &str, x: f64, w: f64| {
        shown(
            json!({ "id": id, "maxclass": "live.comment", "text": text, "numinlets": 1, "numoutlets": 0, "patching_rect": [600.0, 20.0, w, 18.0] }),
            [x, 0.0, w, HEADER - 2.0],
            &[],
            None,
        )
    };
    let source_x = MAP + 2.0 + UNMAP + 2.0;
    let min_x = source_x + if count > 1 { SOURCE + 2.0 } else { 0.0 };
    let max_x = min_x + AMOUNT + 2.0;
    boxes.push(comment("head-map", "Map", 0.0, MAP));
    if count > 1 {
        boxes.push(comment("head-source", "Source", source_x, SOURCE));
    }
    boxes.push(comment("head-min", "Min", min_x, AMOUNT));
    boxes.push(comment("head-max", "Max", max_x, AMOUNT));
    let lcd = [
        ("bgcolor", "live_lcd_bg"),
        ("bgoncolor", "live_lcd_bg"),
        ("activebgcolor", "live_lcd_bg"),
        ("activebgoncolor", "live_lcd_bg"),
        ("bordercolor", "live_lcd_frame"),
        ("textcolor", "live_lcd_control_fg_zombie"),
        ("activetextcolor", "live_lcd_control_fg"),
        ("activetextoncolor", "live_lcd_control_fg"),
        ("textoffcolor", "live_lcd_control_fg_zombie"),
    ];
    for slot in 1..=slots {
        let y = HEADER + (slot - 1) as f64 * ROW;
        let row = 140.0 + slot as f64 * 120.0;
        let k = |part: &str| format!("{part}-{slot}");
        // The Map button: on while live.map listens for the next parameter clicked; then it shows that parameter.
        boxes.push(shown(
            json!({ "id": k("map"), "varname": format!("Map {slot}"), "maxclass": "live.text", "text": "Map", "texton": "Map", "mode": 1, "appearance": 2,
                "numinlets": 1, "numoutlets": 2, "outlettype": ["", ""], "patching_rect": [40.0, row, 60.0, 15.0] }),
            [0.0, y, MAP, 15.0],
            &lcd,
            Some(parameter(&format!("Map {slot}"), "Map", 2, json!({ "parameter_type": 2, "parameter_enum": ["off", "on"], "parameter_mmax": 1 }))),
        ));
        let mut unmap = shown(
            json!({ "id": k("unmap"), "varname": format!("Unmap {slot}"), "maxclass": "live.text", "text": "×", "texton": "×", "mode": 0,
                "numinlets": 1, "numoutlets": 2, "outlettype": ["", ""], "patching_rect": [200.0, row, 20.0, 15.0] }),
            [MAP + 2.0, y, UNMAP, 15.0],
            &[],
            Some(parameter(
                &format!("Unmap {slot}"),
                "Unmap",
                2,
                json!({ "parameter_type": 2, "parameter_enum": ["off", "on"], "parameter_mmax": 1 }),
            )),
        );
        unmap["hidden"] = json!(1);
        boxes.push(unmap);
        if count > 1 {
            let names: Vec<&String> = sources.iter().take(count).collect();
            boxes.push(shown(
                json!({ "id": k("source"), "varname": format!("Map {slot} Source"), "maxclass": "live.menu", "numinlets": 1, "numoutlets": 3, "outlettype": ["", "", "float"],
                    "patching_rect": [260.0, row, 60.0, 15.0] }),
                [source_x, y, SOURCE, 15.0],
                &[],
                Some(parameter(&format!("Map {slot} Source"), "Source", 1, json!({ "parameter_type": 2, "parameter_enum": names, "parameter_mmax": count - 1, "parameter_initial": [0] }))),
            ));
        }
        for (part, x, initial) in [("Min", min_x, 0), ("Max", max_x, 100)] {
            boxes.push(shown(
                json!({ "id": k(&part.to_lowercase()), "varname": format!("Map {slot} {part}"), "maxclass": "live.numbox", "numinlets": 1, "numoutlets": 2,
                    "outlettype": ["", "float"], "patching_rect": [if part == "Min" { 340.0 } else { 400.0 }, row, 44.0, 15.0] }),
                [x, y, AMOUNT, 15.0],
                &[],
                Some(parameter(
                    &format!("Map {slot} {part}"),
                    part,
                    1,
                    json!({ "parameter_type": 1, "parameter_mmin": 0.0, "parameter_mmax": 100.0, "parameter_initial": [initial], "parameter_unitstyle": 5 }),
                )),
            ));
        }
        // live.map: its mapping kept with the Set (persistence), and never one of this device's own parameters (strict).
        boxes.push(json!({ "id": k("live-map"), "maxclass": "newobj", "text": "live.map @strict 1", "numinlets": 1, "numoutlets": 5,
            "outlettype": ["", "", "", "int", ""], "saved_object_attributes": { "_persistence": 1 }, "patching_rect": [40.0, row + 40.0, 110.0, 22.0] }));
        boxes.push(object(&k("listen"), "prepend mapping", 1, 1, [40.0, row + 20.0]));
        boxes.push(message(&k("forget"), "unmap", [200.0, row + 20.0]));
        boxes.push(object(&k("pressed"), "sel 1", 2, 2, [200.0, row + 2.0]));
        boxes.push(object(&k("lit"), "prepend set", 1, 1, [160.0, row + 70.0]));
        boxes.push(object(&k("named"), "tosymbol", 1, 1, [260.0, row + 70.0]));
        boxes.push(object(&k("blank"), "substitute <none> Map", 2, 2, [260.0, row + 95.0]));
        boxes.push(message(&k("label"), "text $1, texton $1", [260.0, row + 70.0 + 50.0]));
        boxes.push(object(&k("id"), "route id", 2, 2, [400.0, row + 70.0]));
        boxes.push(object(&k("none"), "== 0", 2, 1, [400.0, row + 95.0]));
        boxes.push(message(&k("hide"), "hidden $1", [400.0, row + 120.0]));
        let modulator = if modulate { "live.modulate~" } else { "live.remote~ @normalized 1 @smoothing 0" };
        boxes.push(
            json!({ "id": k("modulate"), "maxclass": "newobj", "text": modulator, "numinlets": 2, "numoutlets": 1, "outlettype": [""],
            "patching_rect": [520.0, row + 70.0, 140.0, 22.0] }),
        );
        for (part, param) in [("min", "min"), ("max", "max"), ("source", "source")] {
            if part == "source" && count < 2 {
                continue;
            }
            boxes.push(object(&k(&format!("to-{part}")), &format!("prepend {param}{slot}"), 1, 1, [600.0 + slot as f64 * 10.0, row]));
            lines.push(cord(&k(part), 0, &k(&format!("to-{part}")), 0));
            lines.push(cord(&k(&format!("to-{part}")), 0, "scale", 0));
        }
        lines.push(cord(&k("map"), 0, &k("listen"), 0));
        lines.push(cord(&k("listen"), 0, &k("live-map"), 0));
        // One slot listens at a time, as in Live's own modulators: a Map pressed tells the device's other slots to stop.
        boxes.push(object(&k("pressed-map"), "sel 1", 2, 2, [100.0, row + 20.0]));
        boxes.push(message(&k("me"), &slot.to_string(), [100.0, row + 45.0]));
        boxes.push(object(&k("tell"), "s ---kumi.map.listening", 1, 0, [100.0, row + 70.0]));
        boxes.push(object(&k("told"), "r ---kumi.map.listening", 0, 1, [160.0, row + 20.0]));
        boxes.push(object(&k("another"), &format!("sel {slot}"), 2, 2, [160.0, row + 45.0]));
        boxes.push(message(&k("stop"), "0", [160.0, row + 70.0]));
        lines.push(cord(&k("map"), 0, &k("pressed-map"), 0));
        lines.push(cord(&k("pressed-map"), 0, &k("me"), 0));
        lines.push(cord(&k("me"), 0, &k("tell"), 0));
        lines.push(cord(&k("told"), 0, &k("another"), 0));
        lines.push(cord(&k("another"), 1, &k("stop"), 0));
        lines.push(cord(&k("stop"), 0, &k("listen"), 0));
        lines.push(cord(&k("unmap"), 0, &k("pressed"), 0));
        lines.push(cord(&k("pressed"), 0, &k("forget"), 0));
        lines.push(cord(&k("forget"), 0, &k("live-map"), 0));
        // While live.map listens, the Map button stays lit; then it shows the parameter's name, or Map again.
        lines.push(cord(&k("live-map"), 3, &k("lit"), 0));
        lines.push(cord(&k("lit"), 0, &k("map"), 0));
        lines.push(cord(&k("live-map"), 2, &k("blank"), 0));
        // substitute sends what it changed from its left outlet and anything else from its right.
        lines.push(cord(&k("blank"), 0, &k("named"), 0));
        lines.push(cord(&k("blank"), 1, &k("named"), 0));
        lines.push(cord(&k("named"), 0, &k("label"), 0));
        lines.push(cord(&k("label"), 0, &k("map"), 0));
        // The parameter's id goes to the slot's modulator; × shows only while the slot has one.
        lines.push(cord(&k("live-map"), 1, &k("modulate"), 1));
        lines.push(cord(&k("live-map"), 1, &k("id"), 0));
        lines.push(cord(&k("id"), 0, &k("none"), 0));
        lines.push(cord(&k("none"), 0, &k("hide"), 0));
        lines.push(cord(&k("hide"), 0, &k("unmap"), 0));
        lines.push(cord("scale", slot - 1, &k("modulate"), 0));
    }
    let patcher = json!({
        "fileversion": 1,
        "appversion": { "major": 9, "minor": 1, "revision": 5, "architecture": "x64", "modernui": 1 },
        "classnamespace": "box",
        "rect": [100.0, 100.0, 900.0, 1400.0],
        "openinpresentation": 1,
        "default_fontsize": 10.0,
        "gridsize": [8.0, 8.0],
        "boxes": boxes.into_iter().map(|item| json!({ "box": item })).collect::<Vec<_>>(),
        "lines": lines,
    });
    json!({ "id": id, "varname": name, "maxclass": "bpatcher", "embed": 1, "numinlets": count, "numoutlets": 0, "offset": [0.0, 0.0],
        "border": 0, "bgmode": 0, "lockeddragscroll": 0, "viewvisibility": 1, "hidden": 1, "presentation": 1,
        "presentation_rect": [0.0, 0.0, width, height], "patching_rect": [40.0, 40.0, width, height], "patcher": patcher })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_genexpr::Gen;

    #[test]
    fn each_slot_follows_its_source_between_its_min_and_max() {
        // live.modulate~ takes −1 to 1: Min 0 and Max 100 sweep all of it.
        let mut gen = Gen::new(&scaler(2, 3, true), false);
        gen.set("min1", 25.0);
        gen.set("max1", 75.0);
        gen.set("source2", 1.0);
        gen.set("min3", 100.0);
        gen.set("max3", 0.0);
        let out = gen.tick(&[0.5, 1.0], 3);
        assert_eq!(
            out,
            vec![0.0, 1.0, 0.0],
            "slot 1 halfway between 25% and 75%; slot 2 follows source 2 to the top; slot 3 runs backwards"
        );
        let clamped = gen.tick(&[2.0, -1.0], 3);
        assert_eq!(clamped, vec![0.5, -1.0, -1.0], "a source past 0 to 1 is held at its end (slot 3's top is its Max, 0%)");
        assert_eq!(Gen::new(&scaler(1, 8, true), false).tick(&[0.0], 8), vec![-1.0; 8], "one source, every slot at the bottom");
        // live.remote~ takes the parameter's range, 0 to 1.
        let mut remote = Gen::new(&scaler(1, 1, false), false);
        remote.set("min1", 20.0);
        remote.set("max1", 60.0);
        assert_eq!(remote.tick(&[0.5], 1), vec![0.4]);
    }

    #[test]
    fn the_page_holds_its_slots_controls_and_modulators() {
        let page = page("obj-mod", "kumi.map", &["LFO 1".into(), "LFO 2".into()], 8, true);
        let boxes = page["patcher"]["boxes"].as_array().unwrap();
        let count = |text: &str| boxes.iter().filter(|entry| entry["box"]["text"] == text).count();
        assert_eq!(count("live.map @strict 1"), 8);
        assert_eq!(count("live.modulate~"), 8);
        assert!(boxes
            .iter()
            .filter(|entry| entry["box"]["text"] == "live.map @strict 1")
            .all(|entry| entry["box"]["saved_object_attributes"]["_persistence"] == 1));
        let parameters: Vec<&str> =
            boxes.iter().filter_map(|entry| entry["box"]["saved_attribute_attributes"]["valueof"]["parameter_longname"].as_str()).collect();
        assert_eq!(parameters.len(), 8 * 5);
        assert!(parameters.contains(&"Map 3 Min") && parameters.contains(&"Map 8 Source") && parameters.contains(&"Unmap 1"));
        let unique: std::collections::HashSet<&&str> = parameters.iter().collect();
        assert_eq!(unique.len(), parameters.len(), "every parameter has a name of its own");
        // Not one is automated: the Map buttons are hidden from Live, the rest stored with the Set only.
        assert!(boxes
            .iter()
            .filter_map(|entry| entry["box"]["saved_attribute_attributes"]["valueof"]["parameter_invisible"].as_u64())
            .all(|invisible| invisible >= 1));
        assert_eq!(page["hidden"], 1);
        assert_eq!(page["numinlets"], 2);
        let remote = super::page("obj-mod", "kumi.map", &["Mod".into()], 4, false);
        assert!(remote["patcher"]["boxes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["box"]["text"] == "live.remote~ @normalized 1 @smoothing 0"));
        let (w, h) = size(1, 8);
        assert!(h <= 169.0 - 6.0 && w < 220.0, "{w} {h}");
    }

    #[test]
    fn a_map_pressed_stops_the_devices_other_slots_listening() {
        let page = page("obj-mod", "kumi.map", &["LFO".into()], 3, true);
        let boxes = page["patcher"]["boxes"].as_array().unwrap();
        let lines = page["patcher"]["lines"].as_array().unwrap();
        let text = |id: &str| {
            boxes.iter().find(|entry| entry["box"]["id"] == id).map(|entry| entry["box"]["text"].as_str().unwrap_or_default().to_string())
        };
        let cord = |from: &str, outlet: u64, to: &str, inlet: u64| {
            lines
                .iter()
                .any(|line| line["patchline"]["source"] == json!([from, outlet]) && line["patchline"]["destination"] == json!([to, inlet]))
        };
        for slot in 1..=3 {
            let k = |part: &str| format!("{part}-{slot}");
            // Pressed (1): the slot's number goes to the device's other slots, named for this copy of the device alone.
            assert_eq!(text(&k("pressed-map")).as_deref(), Some("sel 1"));
            assert!(
                cord(&k("map"), 0, &k("pressed-map"), 0) && cord(&k("pressed-map"), 0, &k("me"), 0) && cord(&k("me"), 0, &k("tell"), 0)
            );
            assert_eq!(text(&k("me")), Some(slot.to_string()));
            assert_eq!(text(&k("tell")).as_deref(), Some("s ---kumi.map.listening"));
            // Another slot's number stops this one's live.map listening; its own passes by.
            assert_eq!(text(&k("told")).as_deref(), Some("r ---kumi.map.listening"));
            assert_eq!(text(&k("another")), Some(format!("sel {slot}")));
            assert!(
                cord(&k("told"), 0, &k("another"), 0) && cord(&k("another"), 1, &k("stop"), 0) && !cord(&k("another"), 0, &k("stop"), 0)
            );
            assert_eq!(text(&k("stop")).as_deref(), Some("0"));
            assert!(cord(&k("stop"), 0, &k("listen"), 0) && cord(&k("listen"), 0, &k("live-map"), 0));
        }
    }
}
