//! A device the model patches with Max's own objects, in Kumi's patch notation ([`super::patch::notation`]). Kumi adds
//! the face (each control an ordinary Live parameter, named in the patch by its name), what the device receives and
//! sends (`in` and `out`), and for sound its output stage; then it lays the patch out and checks it.
//!
//! - A MIDI effect: `in` is the MIDI it receives, as [midiin] sends it (bytes); `out` takes MIDI on to [midiout].
//! - An audio effect: `in.0` and `in.1` are the input's left and right; `out.0` and `out.1` go to Kumi's output stage
//!   (Mix against the dry input, then Output) and on to Live.
//! - An instrument: `in` is the track's MIDI (bytes); `out.0` and `out.1` go to Kumi's output stage (Output).

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use super::amxd::{device_patcher, DevicePatcherOptions, DeviceType, Line, PatchBox};
use super::gen::{gen_box, Face, MIX, OUTPUT, OUTPUT_STAGE_EFFECT, OUTPUT_STAGE_INSTRUMENT};
use super::patch::check::check_with;
use super::patch::notation::{expand, parse, End, Outside};
use super::patch::reference::Reference;
use super::patch::standard::{standard, Level};
use super::patch::{NoFiles, Patcher};
use super::spec::PatchSpec;

/// What a patch's maker answers for: what's in it (its objects and cords, the order its messages go in, the names it
/// shares). Its layout, its face and its parameters are Kumi's.
const THE_PATCH_S: [&str; 3] = ["patch.", "order.", "names."];

/// A newobj box of Kumi's own, with its in- and outlets and what each outlet sends.
fn object(id: &str, text: &str, ins: usize, outs: usize, outlettype: &[&str]) -> PatchBox {
    PatchBox::new(json!({ "id": id, "maxclass": "newobj", "text": text, "numinlets": ins, "numoutlets": outs, "outlettype": outlettype,
        "patching_rect": [40.0, 40.0, 60.0, 22.0] }))
}

/// The device's patcher, built around the model's patch and checked, or what's wrong with the patch, each said so the
/// model can fix it. `reference` is what Kumi learned of the installed Max (each object's inlets and outlets, which
/// objects there are); without it, the patch is checked for all but those.
pub fn patched_device(spec: &PatchSpec, reference: Option<&Reference>) -> Result<Value, Vec<String>> {
    let said = |problems: Vec<String>| problems.into_iter().map(|problem| format!("patch {problem}")).collect::<Vec<_>>();
    let notation = parse(&spec.patch).map_err(said)?;
    let mut boxes: Vec<PatchBox> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let kumis = match spec.kind {
        DeviceType::MidiEffect => vec![],
        DeviceType::AudioEffect => vec![&*MIX, &*OUTPUT],
        DeviceType::Instrument => vec![&*OUTPUT],
    };
    let mut face = Face::new(spec.controls.len() + kumis.len());
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
    let controls: BTreeMap<String, String> =
        spec.controls.iter().map(|control| (control.name().to_string(), face.add_alone(control))).collect();
    for control in kumis {
        let param = if control.name() == "Mix" { "kumi_mix" } else { "kumi_output" };
        face.add(control, param, &["obj-out"]);
    }
    let outside = Outside {
        boxes: HashMap::from([("in".to_string(), "obj-in".to_string()), ("out".to_string(), "obj-out".to_string())]),
        controls,
        reference,
    };
    let (patch_boxes, patch_lines) = expand(&notation, &outside).map_err(said)?;
    let mut problems: Vec<String> = Vec::new();
    // A control nothing takes from does nothing; an out nothing reaches is a device that's silent.
    for control in &spec.controls {
        let wired = notation
            .cords
            .iter()
            .any(|link| [&link.from, &link.to].iter().any(|end| matches!(end, End::Control(name) if name == control.name())));
        if !wired {
            problems.push(format!(
                "patch: \"{}\" isn't wired: take its value from \"{}\" -> … (or leave the control out).",
                control.name(),
                control.name()
            ));
        }
    }
    let reached =
        |inlet: usize| notation.cords.iter().any(|link| matches!(&link.to, End::Outside(name) if name == "out") && link.inlet == inlet);
    match spec.kind {
        DeviceType::MidiEffect if !reached(0) => {
            problems.push("patch: nothing reaches out, so the device sends no MIDI: end the patch -> out.".into());
        }
        DeviceType::AudioEffect | DeviceType::Instrument => {
            for (inlet, side) in [(0, "left"), (1, "right")] {
                if !reached(inlet) {
                    problems.push(format!(
                        "patch: nothing reaches out.{inlet}, the {side} channel: send it sound (the same as the other side, for mono)."
                    ));
                }
            }
        }
        _ => {}
    }
    for entry in patch_boxes {
        boxes.push(PatchBox::new(entry["box"].clone()));
    }
    for entry in &patch_lines {
        let end = |key: &str| {
            (entry["patchline"][key][0].as_str().unwrap_or("").to_string(), entry["patchline"][key][1].as_u64().unwrap_or(0) as u32)
        };
        let ((from, outlet), (to, inlet)) = (end("source"), end("destination"));
        lines.push(Line::new(&from, outlet, &to, inlet));
    }
    let width = face.width();
    boxes.extend(face.boxes);
    lines.extend(face.lines);
    let document =
        device_patcher(spec.kind, DevicePatcherOptions { title: spec.name.clone(), description: spec.about.clone(), width, boxes, lines });
    let report = check_with(&Patcher::read(&document, &NoFiles), standard(), reference);
    for finding in report.at_least(Level::Warn) {
        if THE_PATCH_S.iter().any(|rule| finding.rule.starts_with(rule)) {
            problems.push(format!("patch: {finding}"));
        }
    }
    if problems.is_empty() {
        Ok(document)
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::patch::check::{check_with, Finding};
    use crate::devices::patch::reference::{Count, Object};
    use crate::devices::spec::{Control, NumericControl, Unit};

    fn knob(name: &str, min: f64, max: f64, unit: Unit) -> Control {
        Control::Number(NumericControl { name: name.into(), min, max, default: min.max(0.0).min(max), unit })
    }

    /// What Max says of the objects these patches use.
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
                    seen: 1,
                    ..Object::default()
                },
            );
        }
        reference
    }

    fn spec(kind: DeviceType, controls: Vec<Control>, patch: &str) -> PatchSpec {
        PatchSpec { kind, name: "Patched".into(), about: "A patch.".into(), controls, patch: patch.into() }
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
        let effect =
            patched_device(&spec(DeviceType::AudioEffect, vec![knob("Gain", -24.0, 24.0, Unit::Db)], GAIN), Some(&reference)).unwrap();
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
        let midi =
            patched_device(&spec(DeviceType::MidiEffect, vec![knob("Semitones", -24.0, 24.0, Unit::St)], transposer), Some(&reference))
                .unwrap();
        assert_eq!(broken(&midi, &reference), Vec::<String>::new());

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
        let instrument = patched_device(&spec(DeviceType::Instrument, vec![], synth), Some(&reference)).unwrap();
        assert_eq!(broken(&instrument, &reference), Vec::<String>::new());
        let voice = Patcher::read(&instrument, &NoFiles).boxes.into_iter().find(|item| item.str("varname") == "voice").unwrap();
        assert_eq!((voice.inlets(), voice.outlets()), (1, 2), "a gen~ with no inputs takes its Params on one inlet; two outputs");
        assert!(patched_device(&spec(DeviceType::Instrument, vec![], synth), None).is_ok(), "without Max, as the cords use each object");
    }

    #[test]
    fn whats_wrong_with_a_patch_comes_back_to_fix() {
        let reference = reference();
        let problems = |kind: DeviceType, controls: Vec<Control>, patch: &str| {
            patched_device(&spec(kind, controls, patch), Some(&reference)).unwrap_err()
        };
        assert_eq!(
            problems(DeviceType::AudioEffect, vec![], "in.0 -> [*~ 1.] -> out.0 =\n"),
            ["patch line 1: expected -> or the end of the line after a box, not =."],
            "what can't be read is said first, with its line"
        );
        let said = problems(
            DeviceType::AudioEffect,
            vec![knob("Drive", 0.0, 1.0, Unit::None)],
            "in.0 -> [*~ 1.] -> out.0\nin.1 -> [mystery~] -> out.0\n",
        );
        assert_eq!(
            said,
            [
                "patch: \"Drive\" isn't wired: take its value from \"Drive\" -> … (or leave the control out).",
                "patch: nothing reaches out.1, the right channel: send it sound (the same as the other side, for mono).",
                "patch: patch.unknown-object: [mystery~] isn't an object this machine's Max knows: check its name (a typo, an object from a package that isn't installed, or an abstraction the device doesn't hold).",
            ]
        );
        // One outlet into [+]'s cold inlet and, through [t], its hot one: which comes first is left to where boxes sit.
        let order = problems(
            DeviceType::MidiEffect,
            vec![knob("Amount", 0.0, 10.0, Unit::None)],
            "add = [+ 0]\n\"Amount\" -> add.1\n\"Amount\" -> [t f] -> add\nadd -> out\nin -> [midiparse]\n",
        );
        assert_eq!(order.len(), 1, "{order:?}");
        assert!(order[0].starts_with("patch: order.fan-out-rejoins: outlet 0 of live.dial \"Amount\" reaches [+ 0] twice"), "{}", order[0]);
        assert_eq!(
            problems(DeviceType::MidiEffect, vec![], "in -> [midiparse]\n"),
            ["patch: nothing reaches out, so the device sends no MIDI: end the patch -> out."]
        );
        assert_eq!(problems(DeviceType::MidiEffect, vec![], "in -> [midiparse\n"), ["patch line 1: a [ isn't closed on its line."]);
    }
}
