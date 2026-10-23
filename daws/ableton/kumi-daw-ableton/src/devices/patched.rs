//! A device the model patches with Max's own objects, in Kumi's patch notation ([`super::patch::notation`]). Kumi adds
//! the face (each control an ordinary Live parameter, named in the patch by its name), what the device receives and
//! sends (`in` and `out`), and for sound its output stage; then it lays the patch out and checks it.
//!
//! - A MIDI effect: `in` is the MIDI it receives, as [midiin] sends it (bytes); `out` takes MIDI on to [midiout].
//! - An audio effect: `in.0` and `in.1` are the input's left and right; `out.0` and `out.1` go to Kumi's output stage
//!   (Mix against the dry input, then Output) and on to Live.
//! - An instrument: `in` is the track's MIDI (bytes); `out.0` and `out.1` go to Kumi's output stage (Output).
//!
//! Any kind can also send:
//! - `mod`: modulation, a signal from 0 to 1 (`mod.1` and on for more sources). Kumi gives the device a Map page of
//!   slots that map it to parameters anywhere in Live ([`super::mapping`]).
//! - `playhead`: the playing step's index, from 0, which lights it on the face's step grid ([`super::look`]).

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use super::amxd::{device_patcher, DevicePatcherOptions, DeviceType, Line, PatchBox};
use super::findings::Findings;
use super::gen::{gen_box, Face, MIX, OUTPUT, OUTPUT_STAGE_EFFECT, OUTPUT_STAGE_INSTRUMENT};
use super::look;
use super::mapping;
use super::patch::check::check_with;
use super::patch::notation::{expand, parse, ports_had, End, Outside};
use super::patch::reference::Reference;
use super::patch::standard::{standard, Level};
use super::patch::{NoFiles, Patcher};
use super::spec::PatchSpec;

/// What a patch's maker answers for: what's in it (its objects and cords, the order its messages go in, the names it
/// shares, how it times its notes, whether it maps parameters itself). Its layout, its face and its parameters are
/// Kumi's.
const THE_PATCH_S: [&str; 5] = ["patch.", "order.", "names.", "timing.", "map."];

/// A [v8ui]'s size on the face: as tall as the face has room for.
const VIEW: [f64; 2] = [160.0, 150.0];
/// Where the patch's playhead and modulation go: the face's step numbers, and the Map page.
const PLAYHEAD: &str = "obj-playhead";
const MAP_PAGE: &str = "obj-mod";
/// The Map page's scripting name, by which its tab shows it.
const MAP_NAME: &str = "kumi.map";

/// A newobj box of Kumi's own, with its in- and outlets and what each outlet sends.
fn object(id: &str, text: &str, ins: usize, outs: usize, outlettype: &[&str]) -> PatchBox {
    PatchBox::new(json!({ "id": id, "maxclass": "newobj", "text": text, "numinlets": ins, "numoutlets": outs, "outlettype": outlettype,
        "patching_rect": [40.0, 40.0, 60.0, 22.0] }))
}

/// The device's patcher, built around the model's patch the nearest way it can be and checked, and what was found, each
/// said so the model can fix it: a line that can't be read is left out, and a cord to a port a box hasn't got too, as
/// Max leaves it. `reference` is what Kumi learned of the installed Max (each object's inlets and outlets, which objects
/// there are); without it, the patch is checked for all but those.
pub fn patched_device(spec: &PatchSpec, reference: Option<&Reference>) -> (Value, Findings) {
    let (notation, mut found) = parse(&spec.patch);
    let mut boxes: Vec<PatchBox> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let kumis = match spec.kind {
        DeviceType::MidiEffect => vec![],
        DeviceType::AudioEffect => vec![&*MIX, &*OUTPUT],
        DeviceType::Instrument => vec![&*OUTPUT],
    };
    let mut face = Face::new();
    // How many sources the patch sends to mod (mod.0, mod.1…): a Map page for any.
    let sources = notation
        .cords
        .iter()
        .filter_map(|link| matches!(&link.to, End::Outside(name) if name == "mod").then_some(link.inlet + 1))
        .max()
        .unwrap_or(0)
        .min(mapping::MOST_SOURCES);
    match spec.kind {
        DeviceType::MidiEffect => {
            boxes.push(object("obj-in", "midiin", 1, 1, &["int"]));
            boxes.push(object("obj-out", "midiout", 1, 0, &[]));
        }
        DeviceType::AudioEffect => {
            // Kumi's output stage takes the patch's sound on its first two inlets and the dry input on the other two.
            boxes.push(object("obj-in", "plugin~ 1 2", 1, 2, &["signal", "signal"]));
            boxes.push(gen_box("obj-out", OUTPUT_STAGE_EFFECT, [40.0, 300.0, 300.0, 22.0]));
            boxes.push(object("obj-plugout", "plugout~ 1 2", 2, 0, &[]));
            for channel in [0u32, 1] {
                lines.push(Line::new("obj-in", channel, "obj-out", channel + 2));
                lines.push(Line::new("obj-out", channel, "obj-plugout", channel));
            }
        }
        DeviceType::Instrument => {
            boxes.push(object("obj-in", "midiin", 1, 1, &["int"]));
            boxes.push(gen_box("obj-out", OUTPUT_STAGE_INSTRUMENT, [40.0, 300.0, 300.0, 22.0]));
            boxes.push(object("obj-plugout", "plugout~ 1 2", 2, 0, &[]));
            for channel in [0u32, 1] {
                lines.push(Line::new("obj-out", channel, "obj-plugout", channel));
            }
        }
    }
    let mut controls: BTreeMap<String, String> =
        spec.controls.iter().map(|control| (control.name().to_string(), face.add_alone(control))).collect();
    // A step lane the patch reads by its name ("Gate" for "Gate 1" … "Gate 16"): the step's index in, that step's value
    // out. Its controls' values gather in a [pak], which a [zl.lookup] reads by index. The lanes read this way, or every
    // lane when the patch lights the playhead, are the face's step grid.
    let named: Vec<&str> = notation
        .cords
        .iter()
        .flat_map(|link| [&link.from, &link.to])
        .filter_map(|end| match end {
            End::Control(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let lights = notation.cords.iter().any(|link| matches!(&link.to, End::Outside(name) if name == "playhead"));
    let candidates = look::lanes(&spec.controls);
    let read: Vec<String> = candidates
        .iter()
        .map(|(lane, _)| lane.clone())
        .filter(|lane| named.contains(&lane.as_str()) && !controls.contains_key(lane))
        .collect();
    let grid: Vec<String> =
        if read.is_empty() && lights { candidates.iter().map(|(lane, _)| lane.clone()).collect() } else { read.clone() };
    for (at, (lane, members)) in candidates.into_iter().enumerate() {
        if !read.contains(&lane) {
            continue;
        }
        let whole = members.iter().all(|index| !matches!(spec.controls[*index], super::spec::Control::Number(_)));
        let zero = if whole { "0" } else { "0." };
        let (gather, read) = (format!("obj-lane-{}", at + 1), format!("obj-lane-{}-step", at + 1));
        boxes.push(object(&gather, &format!("pak {}", vec![zero; members.len()].join(" ")), members.len(), 1, &[""]));
        boxes.push(object(&read, "zl.lookup", 2, 2, &["", ""]));
        for (step, index) in members.iter().enumerate() {
            lines.push(Line::new(&controls[spec.controls[*index].name()], 0, &gather, step as u32));
        }
        lines.push(Line::new(&gather, 0, &read, 1));
        controls.insert(lane, read);
    }
    for control in kumis {
        let param = if control.name() == "Mix" { "kumi_mix" } else { "kumi_output" };
        face.add(control, param, &["obj-out"]);
    }
    let outside = Outside {
        boxes: HashMap::from([
            ("in".to_string(), "obj-in".to_string()),
            ("out".to_string(), "obj-out".to_string()),
            ("playhead".to_string(), PLAYHEAD.to_string()),
            ("mod".to_string(), MAP_PAGE.to_string()),
        ]),
        controls,
        reference,
        output_stage: spec.kind != DeviceType::MidiEffect,
    };
    let expanded = expand(&notation, &outside);
    found.append(expanded.found);
    let mut patch_boxes = expanded.boxes;
    // The ports of Kumi's own boxes (in, out, the face's controls, the playhead and the Map page), for the cords that
    // reach them, and what the patch calls each.
    let mut own: HashMap<String, (usize, usize, String)> = HashMap::new();
    own.insert(PLAYHEAD.into(), (1, 0, "playhead".into()));
    own.insert(MAP_PAGE.into(), (sources.max(1), 0, "mod".into()));
    for item in boxes.iter().chain(&face.boxes) {
        let (Some(id), Some(ins), Some(outs)) = (
            item.item.get("id").and_then(Value::as_str),
            item.item.get("numinlets").and_then(Value::as_u64),
            item.item.get("numoutlets").and_then(Value::as_u64),
        ) else {
            continue;
        };
        let called = match id {
            "obj-in" => "in".to_string(),
            "obj-out" => "out".to_string(),
            _ => outside
                .controls
                .iter()
                .find(|(_, control)| control.as_str() == id)
                .map_or(id.to_string(), |(name, _)| format!("\"{name}\"")),
        };
        own.insert(id.to_string(), (ins as usize, outs as usize, called));
    }
    for (entry, line) in &expanded.lines {
        let end = |key: &str| {
            (entry["patchline"][key][0].as_str().unwrap_or("").to_string(), entry["patchline"][key][1].as_u64().unwrap_or(0) as u32)
        };
        let ((from, outlet), (to, inlet)) = (end("source"), end("destination"));
        if let Some((_, outs, called)) = own.get(&from).filter(|(_, outs, _)| outlet as usize >= *outs) {
            let has = ports_had("outlet", *outs);
            found.wrong.push(format!(
                "line {line}: a cord leaves outlet {outlet} of {called}, which has {has}, so Kumi left it out, as Max would."
            ));
            continue;
        }
        if let Some((ins, _, called)) = own.get(&to).filter(|(ins, _, _)| inlet as usize >= *ins) {
            let has = ports_had("inlet", *ins);
            found
                .wrong
                .push(format!("line {line}: a cord enters inlet {inlet} of {called}, which has {has}, so Kumi left it out, as Max would."));
            continue;
        }
        lines.push(Line::new(&from, outlet, &to, inlet));
    }
    // A control nothing takes from does nothing, and an out nothing reaches is a device that's silent: read from the cords
    // as they're made, so a cord left out reaches nothing.
    for control in &spec.controls {
        let id = outside.controls.get(control.name()).map(String::as_str).unwrap_or_default();
        let wired = lines.iter().any(|line| line.patchline.source.0 == id || line.patchline.destination.0 == id);
        if !wired {
            found.wrong.push(format!(
                "patch: \"{}\" isn't wired, so it changes nothing: take its value from \"{}\" -> … (or leave the control out).",
                control.name(),
                control.name()
            ));
        }
    }
    let reached = |inlet: u32| lines.iter().any(|line| line.patchline.destination.0 == "obj-out" && line.patchline.destination.1 == inlet);
    match spec.kind {
        DeviceType::MidiEffect if !reached(0) => {
            found.wrong.push("patch: nothing reaches out, so the device sends no MIDI: end the patch -> out.".into());
        }
        DeviceType::AudioEffect | DeviceType::Instrument => {
            for (inlet, side) in [(0, "left"), (1, "right")] {
                if !reached(inlet) {
                    found.wrong.push(format!(
                        "patch: nothing reaches out.{inlet}, the {side} channel, so it's silent: send it sound (the same as the other side, for mono)."
                    ));
                }
            }
        }
        _ => {}
    }
    // What draws (a [v8ui]) is on the face too, in a section of its own; a patch sending modulation gets the Map page.
    let pictures: Vec<usize> =
        patch_boxes.iter().enumerate().filter(|(_, entry)| entry["box"]["maxclass"] == "v8ui").map(|(at, _)| at).collect();
    let names: Vec<String> = (0..sources)
        .map(|at| spec.map.sources.get(at).cloned().unwrap_or_else(|| if sources == 1 { "Mod".into() } else { format!("Mod {}", at + 1) }))
        .collect();
    let extras = look::Extras {
        pictures: pictures.iter().map(|_| (VIEW[0], VIEW[1])).collect(),
        map_page: (sources > 0).then(|| mapping::size(sources, spec.map.slots)),
        lanes: grid,
    };
    let laid = face.finish(&extras, PLAYHEAD);
    let mut main: Vec<String> = face.main_page();
    for (picture, rect) in pictures.iter().zip(&laid.pictures) {
        let item = &mut patch_boxes[*picture]["box"];
        item["presentation"] = json!(1);
        item["presentation_rect"] = json!(rect);
        // Its scripting name, by which the page tabs show and hide it.
        if item.get("varname").and_then(Value::as_str).is_none() {
            item["varname"] = json!(format!("kumi-picture-{}", picture + 1));
        }
        main.push(item["varname"].as_str().unwrap_or_default().to_string());
    }
    if let (Some(rect), Some(tabs)) = (laid.map_page, laid.tabs) {
        // live.modulate~ where this Max has it (Live 12's), live.remote~ where it hasn't.
        let modulate = reference.is_none_or(|reference| reference.object("live.modulate~").is_some());
        let mut page = mapping::page(MAP_PAGE, MAP_NAME, &names, spec.map.slots, modulate);
        page["presentation_rect"] = json!(rect);
        boxes.push(PatchBox::new(page));
        let (tab_boxes, tab_cords) = look::pages(tabs, &main, MAP_NAME);
        boxes.extend(tab_boxes.into_iter().map(PatchBox::new));
        lines.extend(tab_cords.into_iter().map(|(from, outlet, to, inlet)| Line::new(&from, outlet, &to, inlet)));
    }
    if !laid.playhead && lines.iter().any(|line| line.patchline.destination.0 == PLAYHEAD) {
        found.wrong.push(
            "patch: playhead lights the face's step grid, and this face has none, so its cords were left out: name the controls of each step for it (\"Gate 1\" … \"Gate 16\"), or leave playhead out."
                .into(),
        );
        lines.retain(|line| line.patchline.destination.0 != PLAYHEAD);
    }
    let width = laid.width;
    for item in patch_boxes.drain(..) {
        boxes.push(PatchBox::new(item["box"].clone()));
    }
    boxes.extend(face.boxes);
    lines.extend(face.lines);
    if !laid.playhead {
        // No step grid: the playhead's own boxes went unused.
        boxes.retain(|item| item.item.get("id").and_then(Value::as_str) != Some(PLAYHEAD));
    }
    let document =
        device_patcher(spec.kind, DevicePatcherOptions { title: spec.name.clone(), description: spec.about.clone(), width, boxes, lines });
    let report = check_with(&Patcher::read(&document, &NoFiles), standard(), reference);
    for finding in report.at_least(Level::Warn) {
        // An object Max doesn't have was said with its line as the patch was read.
        let said = reference.is_some() && finding.rule == "patch.unknown-object";
        if THE_PATCH_S.iter().any(|rule| finding.rule.starts_with(rule)) && !said {
            found.rule(finding.level, format!("patch: {finding}"));
        }
    }
    // In the order of the patch's lines, each said as the patch's ("patch line 4: …"), then what's said of all of it.
    found.by_line();
    for text in found.wrong.iter_mut().chain(&mut found.changed).chain(&mut found.notes).filter(|text| text.starts_with("line ")) {
        text.insert_str(0, "patch ");
    }
    (document, found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::patch::check::{check_with, Finding};
    use crate::devices::patch::notation::SPEAKERS;
    use crate::devices::patch::reference::{Count, Object};
    use crate::devices::spec::{Control, NumericControl, Unit};

    fn knob(name: &str, min: f64, max: f64, unit: Unit) -> Control {
        Control::Number(NumericControl { name: name.into(), min, max, default: min.max(0.0).min(max), unit, place: Default::default() })
    }

    /// What Max says of the objects these patches use, as its help shows them bare and with up to two arguments.
    fn reference() -> Reference {
        let mut reference = Reference::default();
        for (class, inlets, outlets, signal) in [
            ("*~", 2, 1, true),
            ("dbtoa", 1, 1, false),
            ("t", 1, 2, false),
            ("trigger", 1, 2, false),
            ("midiparse", 1, 8, false),
            ("midiformat", 7, 2, false),
            ("unpack", 1, 2, false),
            ("pack", 2, 1, false),
            ("+", 2, 1, false),
            ("prepend", 1, 1, false),
            ("/", 2, 1, false),
            ("midiin", 1, 1, false),
            ("midiout", 1, 0, false),
            ("plugin~", 1, 2, true),
            ("plugout~", 2, 0, false),
            ("gen~", 1, 2, true),
        ] {
            let types = vec![if signal { "signal".to_string() } else { String::new() }; outlets.max(1)];
            reference.objects.insert(
                class.into(),
                Object {
                    inlets: Count::Fixed { count: inlets },
                    outlets: Count::Fixed { count: outlets },
                    outlet_types: types,
                    seen: 3,
                    held: vec![0, 1, 2],
                    ..Object::default()
                },
            );
        }
        // As Max's help shows [pipe]: a delay time alone holds two inlets and an outlet; several numbers delayed don't.
        reference.objects.insert(
            "pipe".into(),
            Object {
                inlets: Count::Fixed { count: 2 },
                outlets: Count::Fixed { count: 1 },
                seen: 13,
                held: vec![0, 1, 2],
                unheld: vec![3, 4, 13],
                ..Object::default()
            },
        );
        reference
    }

    fn spec(kind: DeviceType, controls: Vec<Control>, patch: &str) -> PatchSpec {
        PatchSpec { kind, name: "Patched".into(), about: "A patch.".into(), controls, patch: patch.into(), map: Default::default() }
    }

    /// Every rule the checker finds broken in a device, at any level but advice.
    fn broken(document: &Value, reference: &Reference) -> Vec<String> {
        check_with(&Patcher::read(document, &NoFiles), standard(), Some(reference)).at_least(Level::Warn).map(Finding::to_string).collect()
    }

    const GAIN: &str = concat!(
        "left = [*~ 1.]\n",
        "right = [*~ 1.]\n",
        "in.0 -> left -> out.0\n",
        "in.1 -> right -> out.1\n",
        "\"Gain\" -> [dbtoa] -> both\n",
        "both = [t f f]\n",
        "both.0 -> left.1\n",
        "both.1 -> right.1\n",
    );

    #[test]
    fn a_patch_becomes_a_device_with_its_face_its_in_and_out_and_kumis_output_stage() {
        let reference = reference();
        let (effect, found) =
            patched_device(&spec(DeviceType::AudioEffect, vec![knob("Gain", -24.0, 24.0, Unit::Db)], GAIN), Some(&reference));
        assert!(found.is_empty(), "{found:?}");
        let patcher = Patcher::read(&effect, &NoFiles);
        let texts: Vec<&str> = patcher.boxes.iter().map(|item| item.text()).filter(|text| !text.is_empty()).collect();
        for text in ["plugin~ 1 2", "gen~", "plugout~ 1 2", "*~ 1.", "dbtoa", "t f f"] {
            assert!(texts.contains(&text), "{text} in {texts:?}");
        }
        let gain =
            patcher.boxes.iter().find(|item| item.maxclass() == "live.dial" && item.str("varname") == "Gain").expect("the Gain dial");
        let dbtoa = patcher.boxes.iter().find(|item| item.text() == "dbtoa").unwrap();
        assert!(patcher.cords.iter().any(|cord| cord.from == gain.id() && cord.to == dbtoa.id()), "the control's value, as the patch says");
        assert!(
            patcher.boxes.iter().any(|item| item.str("varname") == "Mix")
                && patcher.boxes.iter().any(|item| item.str("varname") == "Output")
        );
        assert_eq!(broken(&effect, &reference), Vec::<String>::new(), "laid out and checked: nothing left broken");

        let transposer = concat!(
            "parse = [midiparse]\n",
            "notes = [unpack 0 0]\n",
            "shift = [+ 0]\n",
            "pair = [pack 0 0]\n",
            "in -> parse -> notes -> shift -> pair -> [midiformat] -> out\n",
            "notes.1 -> pair.1\n",
            "\"Semitones\" -> shift.1\n",
        );
        let (midi, found) =
            patched_device(&spec(DeviceType::MidiEffect, vec![knob("Semitones", -24.0, 24.0, Unit::St)], transposer), Some(&reference));
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(broken(&midi, &reference), Vec::<String>::new());
        // [pipe 0 0 125] delays pitch and velocity: an outlet and an inlet more than Max's help ever shows.
        let echo = concat!(
            "notes = [unpack 0 0]\n",
            "later = [pipe 0 0 125]\n",
            "pair = [pack 0 0]\n",
            "in -> [midiparse] -> notes -> later\n",
            "notes.1 -> later.1\n",
            "later.1 -> pair.1\n",
            "later -> pair -> [midiformat] -> out\n",
        );
        let (echo, found) = patched_device(&spec(DeviceType::MidiEffect, vec![], echo), Some(&reference));
        assert!(found.is_empty(), "{found:?}");
        let later = Patcher::read(&echo, &NoFiles).boxes.into_iter().find(|item| item.str("varname") == "later").unwrap();
        assert_eq!((later.inlets(), later.outlets()), (2, 2), "as its cords use it");
        // What a [v8ui] draws is on the face, beside the controls; the device is as wide as it takes.
        let drawn = "view = [v8ui] { function msg_float(v) { mgraphics.redraw(); } }\n\"Semitones\" -> view\nin -> out\n";
        let (drawn, found) =
            patched_device(&spec(DeviceType::MidiEffect, vec![knob("Semitones", -24.0, 24.0, Unit::St)], drawn), Some(&reference));
        assert!(found.is_empty(), "{found:?}");
        let drawn_patcher = Patcher::read(&drawn, &NoFiles);
        let view = drawn_patcher.boxes.iter().find(|item| item.maxclass() == "v8ui").unwrap();
        let dial = drawn_patcher.boxes.iter().find(|item| item.str("varname") == "Semitones").unwrap();
        assert!(view.shown());
        let (view_rect, dial_rect) = (view.presentation_rect().unwrap(), dial.presentation_rect().unwrap());
        assert_eq!(view_rect.w, VIEW[0]);
        assert!(view_rect.x >= dial_rect.x + dial_rect.w, "the picture in a section of its own, right of the controls");
        assert!(drawn["patcher"]["devicewidth"].as_f64().unwrap() >= view_rect.x + view_rect.w);
        assert_eq!(broken(&drawn, &reference), Vec::<String>::new());

        let synth = concat!(
            "voice = [gen~] {\n",
            "  Param pitch(60); Param level(0);\n",
            "  out1 = cycle(mtof(pitch)) * level * 0.2;\n",
            "  out2 = out1;\n",
            "}\n",
            "notes = [unpack 0 0]\n",
            "in -> [midiparse] -> notes -> [prepend pitch] -> voice\n",
            "notes.1 -> [/ 127.] -> [prepend level] -> voice\n",
            "voice.0 -> out.0\n",
            "voice.1 -> out.1\n",
        );
        let (instrument, found) = patched_device(&spec(DeviceType::Instrument, vec![], synth), Some(&reference));
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(broken(&instrument, &reference), Vec::<String>::new());
        let voice = Patcher::read(&instrument, &NoFiles).boxes.into_iter().find(|item| item.str("varname") == "voice").unwrap();
        assert_eq!((voice.inlets(), voice.outlets()), (1, 2), "a gen~ with no inputs takes its Params on one inlet; two outputs");
        assert!(
            patched_device(&spec(DeviceType::Instrument, vec![], synth), None).1.is_empty(),
            "without Max, as the cords use each object"
        );
    }

    #[test]
    fn a_patchs_own_way_out_goes_through_kumis_output_stage_at_any_depth() {
        let reference = reference();
        // A runaway feedback loop sent to a plugout~ of its own, a dac~, and an ezdac~ inside a [p].
        let patch = concat!(
            "fb = [gen~] { History h(0); h = in1 + h * 1.5; out1 = h; out2 = h; }\n",
            "in.0 -> fb\n",
            "fb -> [plugout~ 1 2]\n",
            "speakers = [dac~ 1 2]\n",
            "fb.1 -> speakers.1\n",
            "inner = [p inner] {\n",
            "  [inlet] -> [ezdac~]\n",
            "}\n",
            "fb -> inner\n",
        );
        for kind in [DeviceType::AudioEffect, DeviceType::Instrument] {
            let (device, found) = patched_device(&spec(kind, vec![], patch), Some(&reference));
            assert_eq!(
                found.changed,
                [
                    "patch line 3: [plugout~ 1 2] was made as out: Kumi's output stage, with its safety net, is the device's only way out.",
                    "patch line 4: [dac~ 1 2] was made as out: Kumi's output stage, with its safety net, is the device's only way out.",
                ]
            );
            assert_eq!(
                found.wrong,
                ["patch line 7: [ezdac~] in a [p] would send sound past Kumi's output stage, the device's only way out, so Kumi left it out: send the sound out of the [p] through an [outlet], then to out."]
            );
            // The one way out at any depth is Kumi's plugout~, and only its output stage reaches it.
            fn ways_out(patcher: &Value, found: &mut Vec<String>) {
                for entry in patcher["boxes"].as_array().into_iter().flatten() {
                    let text = entry["box"]["text"].as_str().unwrap_or_default();
                    if SPEAKERS.iter().any(|speaker| text.split(' ').next() == Some(*speaker)) {
                        found.push(entry["box"]["id"].as_str().unwrap_or_default().to_string());
                    }
                    ways_out(&entry["box"]["patcher"], found);
                }
            }
            let mut found = Vec::new();
            ways_out(&device["patcher"], &mut found);
            assert_eq!(found, ["obj-plugout"]);
            let into_plugout: Vec<&str> = device["patcher"]["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|line| line["patchline"]["destination"][0] == "obj-plugout")
                .map(|line| line["patchline"]["source"][0].as_str().unwrap())
                .collect();
            assert!(!into_plugout.is_empty() && into_plugout.iter().all(|source| *source == "obj-out"), "{into_plugout:?}");
            let into_out: Vec<(String, u64)> = device["patcher"]["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|line| line["patchline"]["destination"][0] == "obj-out")
                .map(|line| {
                    (line["patchline"]["source"][0].as_str().unwrap().to_string(), line["patchline"]["destination"][1].as_u64().unwrap())
                })
                .collect();
            let patched = |id: &str| id.strip_prefix("obj-").is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()));
            let fb = into_out.iter().filter(|(source, _)| patched(source)).map(|(_, inlet)| *inlet).collect::<Vec<_>>();
            assert_eq!(fb, [0, 1], "the gen~ reaches out.0 and out.1 through what was its plugout~ and its dac~");
        }
    }

    /// What's wrong with a MIDI effect made from `patch`, by the rules it breaks ("timing.ms-clock").
    fn rules_broken(patch: &str) -> Vec<String> {
        let (_, found) = patched_device(&spec(DeviceType::MidiEffect, vec![], patch), None);
        found.wrong.iter().filter_map(|text| text.split(": ").nth(1).map(str::to_string)).filter(|rule| rule.contains('.')).collect()
    }

    #[test]
    fn notes_timed_from_a_polled_position_in_milliseconds_off_the_grid_or_through_low_priority_are_wrong() {
        // The old way: plugsync~'s song position made a signal, steps found in gen~, sent out by edge~ and snapshot~.
        let polled = concat!(
            "sync = [plugsync~]\n",
            "heard = [sig~]\n",
            "sync.6 -> heard\n",
            "engine = [gen~] { out1 = in1 > 1; out2 = 60; }\n",
            "heard -> engine\n",
            "starts = [edge~]\n",
            "engine -> starts\n",
            "note = [snapshot~]\n",
            "engine.1 -> note\n",
            "starts -> note\n",
            "note -> [pack 0 100] -> [midiformat] -> out\n",
        );
        assert_eq!(rules_broken(polled), ["timing.polled-position"]);
        let clocked =
            |clock: &str| format!("clock = [{clock}]\nclock -> [makenote 60 100 50] -> pair = [pack 0 0]\npair -> [midiformat] -> out\n");
        assert_eq!(rules_broken(&clocked("metro 125 @active 1")), ["timing.ms-clock"]);
        assert_eq!(rules_broken(&clocked("metro @interval 250 ms")), ["timing.ms-clock"]);
        assert_eq!(rules_broken(&clocked("metro 16n @active 1")), ["timing.off-grid-clock"]);
        assert_eq!(rules_broken(&clocked("qmetro 16n @quantize 16n")), ["timing.low-priority-notes"]);
        assert!(rules_broken(&clocked("metro 16n @quantize 16n @active 1")).is_empty());
        assert!(rules_broken(&clocked("metro @interval 120 ticks @quantize 120 ticks")).is_empty());
        let deferred = "clock = [metro 16n @quantize 16n]\nclock -> [deferlow] -> [makenote 60 100 50] -> pair = [pack 0 0]\npair -> [midiformat] -> out\n";
        assert_eq!(rules_broken(deferred), ["timing.low-priority-notes"]);
        let coded = "clock = [metro 16n @quantize 16n]\nclock -> step = [v8 step] { function bang() { outlet(0, 60); } }\nstep -> [makenote 60 100 50] -> pair = [pack 0 0]\npair -> [midiformat] -> out\n";
        assert_eq!(rules_broken(coded), ["timing.low-priority-notes"]);
    }

    #[test]
    fn milliseconds_and_low_priority_that_time_no_notes_are_fine() {
        // A display refreshed by [qmetro], a [delay] after the device loads, a fixed live.remote~ and MIDI passed
        // through code are all fine: none times notes from a clock.
        let fine = concat!(
            "[qmetro 33] -> [snapshot~] -> [prepend set] -> view = [v8ui] { function msg_float(v) { mgraphics.redraw(); } }\n",
            "[live.thisdevice] -> [delay 100] -> [deferlow] -> (getid) -> volume = [live.path live_set master_track mixer_device volume]\n",
            "remote = [live.remote~]\n",
            "volume -> remote.1\n",
            "[sig~ 0.5] -> remote\n",
            "in -> pass = [v8 pass] { function msg_int(v) { outlet(0, v); } }\n",
            "pass -> out\n",
        );
        assert!(rules_broken(fine).is_empty(), "{:?}", rules_broken(fine));
        // An LFO that sends a controller in milliseconds times no notes.
        let cc = "[metro 10 @active 1] -> [snapshot~] -> [ctlout 1]\n[cycle~ 0.5] -> [snapshot~]\nin -> out\n";
        assert!(rules_broken(cc).is_empty(), "{:?}", rules_broken(cc));
        let formatted =
            "form = [midiformat]\nclock = [metro 125 @active 1]\nclock -> [i 74] -> cc = [pack 74 0]\ncc -> form.2\nform -> out\n";
        assert!(rules_broken(formatted).is_empty(), "a controller formatted for midiformat's controller inlet");
        // Kumi's own clock and notes: plugsync~ and a sig~ inside them, all made the tested way.
        let kit = concat!(
            "clock = [kumi.clock 16n 8n]\n",
            "\"Rate\" -> clock\n",
            "notes = [kumi.notes]\n",
            "clock -> [+ 60] -> notes\n",
            "clock.1 -> [* 0.5] -> notes.2\n",
            "notes -> out\n",
        );
        let (_, found) = patched_device(
            &spec(
                DeviceType::MidiEffect,
                vec![Control::Choice(crate::devices::spec::ChoiceControl {
                    name: "Rate".into(),
                    options: vec!["1/16".into(), "1/8".into()],
                    default: "1/16".into(),
                    place: Default::default(),
                })],
                kit,
            ),
            None,
        );
        assert!(found.wrong.is_empty(), "{:?}", found.wrong);
    }

    #[test]
    fn a_step_lane_is_read_by_its_name_and_its_steps_need_no_cords_of_their_own() {
        let mut controls: Vec<Control> = Vec::new();
        for step in 1..=8 {
            controls.push(Control::Switch(crate::devices::spec::SwitchControl {
                name: format!("Gate {step}"),
                default: true,
                place: Default::default(),
            }));
        }
        for step in 1..=8 {
            controls.push(knob(&format!("Note {step}"), 0.0, 127.0, Unit::None));
        }
        let patch = concat!(
            "clock = [kumi.clock 16n @length 8]\n",
            "clock -> step = [t i i i]\n",
            "step.2 -> \"Gate\" -> open = [gate]\n",
            "step.1 -> playhead\n",
            "step -> \"Note\" -> open.1\n",
            "open -> notes = [kumi.notes]\n",
            "clock.1 -> [* 0.5] -> notes.2\n",
            "notes -> out\n",
        );
        let (device, found) = patched_device(&spec(DeviceType::MidiEffect, controls, patch), None);
        assert!(found.wrong.is_empty(), "{:?}", found.wrong);
        let patcher = Patcher::read(&device, &NoFiles);
        let gather: Vec<&str> = patcher.boxes.iter().map(|item| item.text()).filter(|text| text.starts_with("pak ")).collect();
        assert_eq!(gather, ["pak 0 0 0 0 0 0 0 0", "pak 0. 0. 0. 0. 0. 0. 0. 0."], "a switch lane's values are whole, a number lane's not");
        let note_8 = patcher.boxes.iter().find(|item| item.str("varname") == "Note 8").unwrap();
        let into: Vec<usize> = patcher.cords.iter().filter(|cord| cord.from == note_8.id()).map(|cord| cord.inlet).collect();
        assert_eq!(into, [7], "step 8 into the lane's eighth place");
        // The playhead lights the step grid: its step numbers are on the face.
        assert!(patcher.cords.iter().any(|cord| cord.to == PLAYHEAD));
        assert_eq!(patcher.boxes.iter().filter(|item| item.maxclass() == "live.text" && item.str("text") == "8").count(), 1);
    }

    #[test]
    fn a_device_that_maps_parameters_itself_is_pointed_to_kumis_mapping_page() {
        let mapped = "map = [live.map]\n(mapping 1) -> map\nmap.1 -> remote = [live.remote~]\n[sig~ 0.5] -> remote\nin -> out\n";
        assert_eq!(rules_broken(mapped), ["map.own-mapping"]);
        let watched = "watch = [v8 watch] { const api = new LiveAPI(null, \"live_set view selected_parameter\"); }\nin -> out\n";
        assert_eq!(rules_broken(watched), ["map.own-mapping"]);
    }

    #[test]
    fn a_patch_is_always_made_and_whats_wrong_comes_back_beside_it() {
        let reference = reference();
        // The device is made every time (with Kumi's own boxes, in, out and the face), and what's wrong is said by line.
        let made = |kind: DeviceType, controls: Vec<Control>, patch: &str| {
            let (document, found) = patched_device(&spec(kind, controls, patch), Some(&reference));
            let patched = |id: &str| id.strip_prefix("obj-").is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()));
            let made = Patcher::read(&document, &NoFiles).boxes.iter().filter(|item| patched(item.id())).count();
            assert_eq!(made, parse(patch).0.boxes.len(), "every box the patch's readable lines make is in the device");
            found
        };
        let unread = made(DeviceType::AudioEffect, vec![], "in.0 -> [*~ 1.] -> out.0 =\nin.1 -> [*~ 1.] -> out.1\n");
        assert_eq!(
            unread.wrong,
            [
                "patch line 1: expected -> or the end of the line after a box, not =, so Kumi left the line out.",
                "patch: nothing reaches out.0, the left channel, so it's silent: send it sound (the same as the other side, for mono).",
            ],
            "what can't be read is left out and said first, with its line; the rest is made"
        );
        let said = made(
            DeviceType::AudioEffect,
            vec![knob("Drive", 0.0, 1.0, Unit::None)],
            "in.0 -> [*~ 1.] -> out.0\nin.1 -> [mystery~] -> out.0\n",
        );
        assert_eq!(
            said.wrong,
            [
                "patch line 2: [mystery~] isn't an object in this Max, so Max will show [mystery~] as a broken box: check its name (make_device with objects can look one up).",
                "patch: \"Drive\" isn't wired, so it changes nothing: take its value from \"Drive\" -> … (or leave the control out).",
                "patch: nothing reaches out.1, the right channel, so it's silent: send it sound (the same as the other side, for mono).",
            ]
        );
        // One outlet into [+]'s cold inlet and, through [t], its hot one: which comes first is left to where boxes sit.
        let order = made(
            DeviceType::MidiEffect,
            vec![knob("Amount", 0.0, 10.0, Unit::None)],
            "add = [+ 0]\n\"Amount\" -> add.1\n\"Amount\" -> [t f] -> add\nadd -> out\nin -> [midiparse]\n",
        );
        assert_eq!(order.wrong.len(), 1, "{order:?}");
        assert!(
            order.wrong[0].starts_with("patch: order.fan-out-rejoins: outlet 0 of live.dial \"Amount\" reaches [+ 0] twice"),
            "{}",
            order.wrong[0]
        );
        assert_eq!(
            made(DeviceType::MidiEffect, vec![], "in -> [midiparse]\n").wrong,
            ["patch: nothing reaches out, so the device sends no MIDI: end the patch -> out."]
        );
        assert_eq!(
            made(DeviceType::MidiEffect, vec![], "in -> [midiparse\nin -> out\n").wrong,
            ["patch line 1: a [ isn't closed on its line, so Kumi left the line out."]
        );
        // A cord to a port a box hasn't got is left out, as Max leaves it: Kumi's own boxes and the patch's alike.
        let ports = made(DeviceType::MidiEffect, vec![], "add = [+ 0]\nin.1 -> out\nin -> add.2\nin -> add -> out\n");
        assert_eq!(
            ports.wrong,
            [
                "patch line 2: a cord leaves outlet 1 of in, which has only outlet 0, so Kumi left it out, as Max would.",
                "patch line 3: a cord enters inlet 2 of [+ 0], which has inlets 0 to 1, so Kumi left it out, as Max would.",
            ]
        );
        // What's said of the cords is of those made: one left out reaches nothing.
        let unreached = made(
            DeviceType::MidiEffect,
            vec![knob("Amount", 0.0, 10.0, Unit::None)],
            "add = [+ 0]\nin -> add\nadd.1 -> out\n\"Amount\" -> add.3\n",
        );
        assert_eq!(
            unreached.wrong,
            [
                "patch line 3: a cord leaves outlet 1 of [+ 0], which has only outlet 0, so Kumi left it out, as Max would.",
                "patch line 4: a cord enters inlet 3 of [+ 0], which has inlets 0 to 1, so Kumi left it out, as Max would.",
                "patch: \"Amount\" isn't wired, so it changes nothing: take its value from \"Amount\" -> … (or leave the control out).",
                "patch: nothing reaches out, so the device sends no MIDI: end the patch -> out.",
            ]
        );
    }
}
