//! Whole screens of the full-screen app as Kumi 1.18.0 draws them for Live, at 120×36 and 160×40: FOCUS, NOW and
//! HISTORY with each picture, the panels, the disconnect notices and the dashboards. Each screen is kept as plain text
//! in tests/goldens/tui (one file per scene and size, trailing spaces trimmed), and FOCUS's scenes keep their colours
//! too (`.styles.txt`, each row's runs of styles); `KUMI_BLESS=1` writes them again.
//!
//! The same bytes on every machine and at any moment: times are given relative to now (saved 3 hours ago, a job
//! started 65.02 s ago, a goal 185.02 s in), so the app's own clock only adds the milliseconds a scene takes; the
//! activity glyphs that turn with that clock go through `finish_steady`; icons, colours and pictures are fixed.
#[path = "support/models.rs"]
mod models;
#[path = "support/tui_app.rs"]
mod support;
use futures::FutureExt;
use kumi::tui::activity::{activity_glyph, Activity};
use serde_json::{json, Value};
use std::{cell::Cell, rc::Rc};
use support::*;

const SIZES: [(i32, i32); 2] = [(120, 36), (160, 40)];
const HOUR: i64 = 3_600_000;

fn golden(name: &str, actual: &str) {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/tui").join(name);
    // Nothing of this machine's: no CR, no home or temp folder.
    assert!(!actual.contains('\r'), "{name} has a CR in it");
    let home = |key: &str| std::path::PathBuf::from(std::env::var_os(key).unwrap_or_default());
    for folder in [std::env::temp_dir(), home("HOME"), home("USERPROFILE")] {
        let folder = folder.display().to_string();
        assert!(folder.len() < 2 || !actual.contains(folder.trim_end_matches(['/', '\\'])), "{name} names {folder}");
    }
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&file)
        .unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()))
        .replace("\r\n", "\n");
    if actual != expected {
        panic!("{} changed: check the difference, then run this test with KUMI_BLESS=1\n{}", file.display(), difference(&expected, actual));
    }
}

/// Where `actual` first parts from `expected`, with the lines around it: `-` expected, `+` actual, by line number.
fn difference(expected: &str, actual: &str) -> String {
    let (expected, actual): (Vec<&str>, Vec<&str>) = (expected.split('\n').collect(), actual.split('\n').collect());
    let rows = expected.len().max(actual.len());
    let differ: Vec<usize> = (0..rows).filter(|&row| expected.get(row) != actual.get(row)).collect();
    let first = differ.first().copied().unwrap_or(0);
    let mut out = vec![format!(
        "{} differing lines, first at line {}: {}",
        differ.len(),
        first + 1,
        differ.iter().take(12).map(|row| (row + 1).to_string()).collect::<Vec<_>>().join(", ")
    )];
    for row in first.saturating_sub(3)..(first + 4).min(rows) {
        match (expected.get(row), actual.get(row)) {
            (Some(e), Some(a)) if e == a => out.push(format!("  {:>4}  {e}", row + 1)),
            (e, a) => {
                out.push(format!("- {:>4}  {}", row + 1, e.copied().unwrap_or("(no line)")));
                out.push(format!("+ {:>4}  {}", row + 1, a.copied().unwrap_or("(no line)")));
            }
        }
    }
    out.join("\n")
}

/// What a scene shows: its screen, and its colours when they're kept.
struct Shot {
    lines: Vec<String>,
    styles: Option<String>,
}

/// One scene at each size: the body opens a harness, drives it and gives back its screen.
macro_rules! scene {
    ($name:ident, |$width:ident, $height:ident| $body:block) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new()
                .run_until(async {
                    for ($width, $height) in SIZES {
                        let shot: Shot = $body;
                        let name = format!("{}.{}x{}", stringify!($name).replace('_', "-"), $width, $height);
                        golden(&format!("{name}.txt"), &text(&shot.lines));
                        if let Some(styles) = shot.styles {
                            golden(&format!("{name}.styles.txt"), &styles);
                        }
                    }
                })
                .await;
        }
    };
}

fn text(lines: &[String]) -> String {
    lines.iter().map(|line| line.trim_end_matches(' ')).collect::<Vec<_>>().join("\n") + "\n"
}

/// Each row's styles as runs ("3×12" is style 3 for 12 cells), under the styles they number (foreground, background,
/// then bold, dim, italic, underline and inverse).
fn styles(rows: &[Vec<String>]) -> String {
    let mut legend: Vec<&str> = vec![];
    let mut lines = vec![];
    for (y, row) in rows.iter().enumerate() {
        let mut runs: Vec<(usize, usize)> = vec![];
        for key in row {
            let id = legend.iter().position(|known| known == key).unwrap_or_else(|| {
                legend.push(key);
                legend.len() - 1
            });
            match runs.last_mut() {
                Some((last, count)) if *last == id => *count += 1,
                _ => runs.push((id, 1)),
            }
        }
        lines.push(format!("{y:>2}: {}", runs.iter().map(|(id, count)| format!("{id}×{count}")).collect::<Vec<_>>().join(" ")));
    }
    let legend = legend.iter().enumerate().map(|(id, key)| format!("{id:>2} = {key}")).collect::<Vec<_>>();
    format!("{}\n\n{}\n", legend.join("\n"), lines.join("\n"))
}

fn now_ms() -> i64 {
    kumi_common::time::now_ms()
}

/// The app with this controller, started and connected to Night Drive.
async fn open(width: i32, height: i32, control: Rc<Control>) -> Harness {
    let h = Harness::with(width, height, control, |_| {});
    h.start().await;
    h.connect();
    h
}

async fn finish(h: Harness) -> Shot {
    let lines = h.screen();
    h.close().await;
    Shot { lines, styles: None }
}

/// The screen and its colours, read again from all the app wrote.
async fn finish_styled(h: Harness) -> Shot {
    let lines = h.screen();
    let mut vt = vt::VirtualTerminal::new(h.output.width.get(), h.output.height.get());
    vt.write(&h.output.text.borrow());
    assert_eq!(vt.lines(), lines);
    h.close().await;
    Shot { lines, styles: Some(styles(&vt.styles)) }
}

/// Activity glyphs turn about ten times a second: the one before each of `texts` is written as the first frame of
/// the first kind that has it, so a screen taken at any moment reads the same.
async fn finish_steady(h: Harness, texts: &[&str]) -> Shot {
    let mut lines = h.screen();
    h.close().await;
    let frames: Vec<Vec<String>> =
        Activity::ALL.iter().map(|kind| (0..12).map(|i| activity_glyph(*kind, i as f64 * 100., false).text).collect()).collect();
    for line in &mut lines {
        let mut chars: Vec<String> = line.chars().map(String::from).collect();
        for text in texts {
            let first = text.chars().next().unwrap().to_string();
            let width = text.chars().count();
            for at in 2..chars.len() {
                if chars[at] == first && chars[at..].len() >= width && chars[at..at + width].concat() == *text {
                    if let Some(kind) = frames.iter().find(|frames| frames.contains(&chars[at - 2])) {
                        chars[at - 2] = kind[0].clone();
                    }
                }
            }
        }
        *line = chars.concat();
    }
    Shot { lines, styles: None }
}

// FOCUS

fn device(reference: &str, name: &str, class: &str, kind: &str) -> Value {
    json!({"ref":reference,"name":name,"className":class,"deviceType":kind})
}

/// An audio track's devices: a plug-in, a Max for Live effect, and a rack with two chains.
fn rack_tree() -> Value {
    let fx = |reference: &str, name: &str, class: &str| device(reference, name, class, "audio_effect");
    json!({"trackRef":"3:track:3","devices":[
        fx("d1","Pro-Q 3","PluginDevice"),
        fx("d3","LFO","MxDeviceAudioEffect"),
        {"ref":"d2","name":"Audio Effect Rack","className":"AudioEffectGroupDevice","canHaveChains":true,"chains":[
            {"ref":"c0","name":"Chain 1","devices":[fx("d2a","Saturator","Saturator"),fx("d2b","EQ Eight","Eq8")]},
            {"ref":"c1","name":"Chain 2","devices":[fx("d2c","Utility","StereoGain")]}]},
        fx("d4","Gate","Gate")]})
}

fn rack_focus() -> Value {
    json!({"type":"focus","focus":{"track":{"name":"4-Audio","color":"#e2b93b","kind":"audio"},"trackRef":"3:track:3","device":"Saturator","chain":"Chain 1","detail":"Device","view":"Session"}})
}

/// A MIDI track's devices: Max for Live MIDI and instrument devices, a MIDI rack, an instrument rack and a plug-in.
fn instrument_tree() -> Value {
    json!({"trackRef":"3:track:5","devices":[
        device("e","Expression Control","MxDeviceMidiEffect","midi_effect"),
        {"ref":"m","name":"MIDI Effect Rack","className":"MidiEffectGroupDevice","canHaveChains":true,"chains":[
            {"ref":"mc","name":"Chain","devices":[device("ma","Arpeggiator","MidiArpeggiator","midi_effect")]}]},
        {"ref":"i","name":"Instrument Rack","className":"InstrumentGroupDevice","canHaveChains":true,"chains":[
            {"ref":"ic","name":"Layer","devices":[device("ia","Drift","Drift","instrument")]}]},
        device("s","Serum 2","PluginDevice","instrument"),
        device("g","Granulator III","MxDeviceInstrument","instrument"),
        device("u","Reverb","Reverb","audio_effect")]})
}

/// A Drum Rack's pads, more than FOCUS has room for.
fn drum_tree() -> Value {
    let pad = |reference: &str, name: &str| json!({"ref":reference,"name":name});
    json!({"trackRef":"3:track:0","devices":[
        {"ref":"r","name":"Drum Rack","className":"DrumGroupDevice","canHaveChains":true,"canHaveDrumPads":true,"chains":[
            {"ref":"p0","name":"Kick","devices":[device("p0a","Drum Sampler","DrumSampler","instrument"),device("p0b","Saturator","Saturator","audio_effect")]},
            pad("p1","Snare"),pad("p2","Closed Hat"),pad("p3","Open Hat"),pad("p4","Clap"),pad("p5","Rim"),pad("p6","Tom"),pad("p7","Ride")]},
        device("u","Glue Compressor","GlueCompressor","audio_effect")]})
}

scene!(focus_welcome, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    finish_styled(h).await
});

scene!(focus_path_device, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Bass","color":"#f59a3c","kind":"midi"},"device":"Operator","detail":"Device","view":"Session","parameter":{"name":"Filter Freq","value":"1.20 kHz","owner":"Operator"}}}));
    finish_styled(h).await
});

scene!(focus_path_clip, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Keys","color":"#5ec1f7","kind":"midi"},"trackRef":"3:track:2","clip":"Verse riff","detail":"Clip","view":"Arrangement","selectedNotes":3}}));
    finish_styled(h).await
});

scene!(focus_path_scene, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Drums","color":"#3b3f8f","kind":"group"},"trackRef":"3:track:0","scene":"Drop","sceneIndex":4,"view":"Session"}}));
    finish_styled(h).await
});

scene!(focus_tree_rack, |width, height| {
    let c = Rc::new(Control::default());
    *c.tree.borrow_mut() = Some(serde_json::from_value(rack_tree()).unwrap());
    let h = open(width, height, c).await;
    h.emit(rack_focus());
    h.wait_for("FOCUS · Device").await;
    finish_styled(h).await
});

scene!(focus_tree_instruments, |width, height| {
    let c = Rc::new(Control::default());
    *c.tree.borrow_mut() = Some(serde_json::from_value(instrument_tree()).unwrap());
    let h = open(width, height, c).await;
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Lead","color":"#b28cff","kind":"midi"},"trackRef":"3:track:5","device":"Arpeggiator","chain":"Chain","detail":"Device","view":"Session"}}));
    h.wait_for("FOCUS · Device").await;
    finish_styled(h).await
});

scene!(focus_tree_drums, |width, height| {
    let c = Rc::new(Control::default());
    *c.tree.borrow_mut() = Some(serde_json::from_value(drum_tree()).unwrap());
    let h = open(width, height, c).await;
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Drums","color":"#ff5c5c","kind":"midi"},"trackRef":"3:track:0","device":"Drum Sampler","chain":"Kick","detail":"Device","view":"Session"}}));
    h.wait_for("FOCUS · Device").await;
    finish_styled(h).await
});

scene!(focus_tree_pinned, |width, height| {
    let c = Rc::new(Control::default());
    *c.tree.borrow_mut() = Some(serde_json::from_value(rack_tree()).unwrap());
    let h = open(width, height, c).await;
    h.emit(rack_focus());
    h.wait_for("FOCUS · Device").await;
    // Into the tree, down to EQ Eight, and pin it.
    h.type_text("\t").await;
    h.type_text("\x1b[B").await;
    h.type_text("\r").await;
    h.wait_for("EQ Eight  ×").await;
    finish_styled(h).await
});

scene!(focus_session, |width, height| {
    let c = Rc::new(Control::default());
    *c.session.borrow_mut() = Some(
        serde_json::from_value(json!({"trackRef":"3:track:1","scene":2,"slots":[
            {"index":0,"clip":{"name":"Intro","audio":false}},
            {"index":1,"clip":{"name":"Verse","audio":false},"playing":true},
            {"index":2,"clip":{"name":"Drop","audio":true},"queued":true},
            {"index":3},
            {"index":4,"clip":{"name":"","audio":false}},
            {"index":5,"clip":{"name":"Outro","audio":true}}]}))
        .unwrap(),
    );
    let h = open(width, height, c).await;
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Bass","color":"#f59a3c","kind":"midi"},"trackRef":"3:track:1","sceneIndex":2,"view":"Session"}}));
    h.wait_for("FOCUS · Session").await;
    finish_styled(h).await
});

scene!(focus_arrangement, |width, height| {
    let c = Rc::new(Control::default());
    *c.arrangement.borrow_mut() = Some(
        serde_json::from_value(json!({"length":128,"position":68,"playing":true,"loop":{"start":64,"length":16,"enabled":true},
            "locators":[{"name":"Intro","position":0},{"name":"Verse","position":32},{"name":"Drop","position":96}]}))
        .unwrap(),
    );
    let h = open(width, height, c).await;
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Bass","color":"#f59a3c","kind":"midi"},"trackRef":"3:track:1","view":"Arrangement"}}));
    h.wait_for("FOCUS · Arrangement").await;
    finish_styled(h).await
});

scene!(focus_clip, |width, height| {
    let c = Rc::new(Control::default());
    let note = |pitch: i32, start: f64, duration: f64, selected: bool| {
        let mut note = json!({"pitch":pitch,"start":start,"duration":duration,"velocity":96});
        if selected {
            note["selected"] = json!(true);
        }
        note
    };
    *c.clip.borrow_mut() = Some(
        serde_json::from_value(json!({"slotRef":"3:clip_slot:2:0","name":"Chords","length":8,"notes":[
            note(48,0.,2.,false),note(55,0.,2.,false),note(60,0.,2.,false),note(64,0.,2.,true),
            note(45,2.,2.,false),note(52,2.,2.,false),note(57,2.,2.,false),note(60,2.,2.,true),
            note(41,4.,2.,false),note(48,4.,2.,false),note(57,4.,2.,false),
            note(43,6.,1.5,false),note(50,6.,1.5,false),note(59,6.,1.5,false),note(67,7.5,0.5,false)]}))
        .unwrap(),
    );
    let h = open(width, height, c).await;
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Keys","color":"#5ec1f7","kind":"midi"},"slotRef":"3:clip_slot:2:0","clip":"Chords","detail":"Clip","view":"Session","selectedNotes":2}}));
    h.wait_for("FOCUS · Clip").await;
    finish_styled(h).await
});

scene!(focus_clip_empty, |width, height| {
    let c = Rc::new(Control::default());
    *c.clip.borrow_mut() = Some(serde_json::from_value(json!({"slotRef":"3:clip_slot:2:1","name":"","length":6,"notes":[]})).unwrap());
    let h = open(width, height, c).await;
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Keys","color":"#5ec1f7","kind":"midi"},"slotRef":"3:clip_slot:2:1","clip":"","detail":"Clip","view":"Session"}}));
    h.wait_for("FOCUS · Clip").await;
    finish_styled(h).await
});

scene!(pin_live_selection, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(json!({"type":"pointed","pin":{"trackRef":"3:track:1","ref":"3:track:1","node":"selection","name":"Bass","trail":[],"siblings":["Drums"],"live":true,"track":"Bass","time":{"fromBeat":64,"toBeat":80}}}));
    finish_styled(h).await
});

scene!(pin_live_clip, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(json!({"type":"pointed","pin":{"trackRef":"3:track:0","ref":"3:arrangement_clip:0:0","node":"clip","name":"Verse riff","trail":["Bass"],"siblings":[],"live":true,"track":"Bass"}}));
    finish_styled(h).await
});

scene!(pin_live_scene, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(
        json!({"type":"pointed","pin":{"trackRef":"","ref":"3:scene:4","node":"scene","name":"Drop","trail":[],"siblings":[],"live":true}}),
    );
    finish_styled(h).await
});

scene!(header_bpm, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(
        json!({"type":"transport","transport":{"playing":true,"tempo":123.5,"beat":8,"at":kumi_common::time::perf_now(),"beatsPerBar":4}}),
    );
    finish(h).await
});

// NOW and HISTORY: each change's picture, as it lands (NOW) and opened in HISTORY.

fn value(name: &str, from: f64, to: f64, from_text: &str, to_text: &str) -> Value {
    json!({"name":name,"from":from,"to":to,"fromText":from_text,"toText":to_text})
}

fn change(id: &str, family: &str, title: &str, extra: Value) -> Value {
    let mut change = json!({"id":id,"family":family,"title":title,"state":"applied","at":1});
    for (key, value) in extra.as_object().unwrap() {
        change[key] = value.clone();
    }
    json!({"type":"change","change":change})
}

/// The change lands, then the keyboard goes into HISTORY and opens it.
async fn picture(width: i32, height: i32, change: Value) -> Shot {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(change);
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    finish(h).await
}

scene!(picture_values, |width, height| {
    let mut asked = value("Resonance", 0.2, 0.35, "20 %", "35 %");
    asked["asked"] = json!("36 %");
    picture(
        width,
        height,
        change(
            "c1",
            "parameter",
            "Operator · Filter Freq 1.20 kHz → 800 Hz, Resonance 20 % → 35 %",
            json!({
            "track":{"name":"Bass","color":"#f59a3c"},
            "picture":{"kind":"values","values":[value("Filter Freq", 0.62, 0.55, "1.20 kHz", "800 Hz"), asked]}}),
        ),
    )
    .await
});

scene!(picture_eq, |width, height| {
    let band = |shape: &str, freq: f64, gain: f64, q: f64| json!({"shape":shape,"freq":freq,"gainDb":gain,"q":q});
    let mut gain = value("3 Gain A", 0.0, -4.0, "0.00 dB", "-4.00 dB");
    gain["asked"] = json!("-4.5 dB");
    picture(
        width,
        height,
        change(
            "c1",
            "parameter",
            "EQ Eight · 3 Gain A 0.00 dB → -4.00 dB",
            json!({
            "track":{"name":"Vocals","color":"#86e3b5"},
            "picture":{"kind":"eq",
                "before":[band("low-cut",80.,0.,0.71),band("bell",1000.,0.,1.),band("high-shelf",8000.,2.,0.71)],
                "after":[band("low-cut",80.,0.,0.71),band("bell",1000.,-4.,1.),band("high-shelf",8000.,2.,0.71)],
                "values":[gain]}}),
        ),
    )
    .await
});

scene!(picture_dynamics, |width, height| {
    picture(
        width,
        height,
        change(
            "c1",
            "parameter",
            "Compressor · Threshold -12.0 dB → -18.0 dB, Ratio 2.00 : 1 → 4.00 : 1",
            json!({
            "track":{"name":"Drums","color":"#ff5c5c"},
            "picture":{"kind":"dynamics",
                "before":{"thresholdDb":-12.0,"ratio":2.0,"kneeDb":6.0,"makeupDb":0.0},
                "after":{"thresholdDb":-18.0,"ratio":4.0,"kneeDb":6.0,"makeupDb":3.0},
                "values":[value("Threshold", 0.6, 0.45, "-12.0 dB", "-18.0 dB"), value("Ratio", 0.3, 0.6, "2.00 : 1", "4.00 : 1")]}}),
        ),
    )
    .await
});

scene!(picture_automation, |width, height| {
    picture(
        width,
        height,
        change(
            "c1",
            "parameter",
            "Filter Freq automation, bars 1–5",
            json!({
            "track":{"name":"Pad","color":"#b28cff"},
            "picture":{"kind":"automation","parameter":"Filter Freq","min":0.0,"max":1.0,
                "before":[[0.0,0.2],[16.0,0.2]],
                "after":[[0.0,0.2],[4.0,0.5],[8.0,0.85],[12.0,0.6],[16.0,0.3]]}}),
        ),
    )
    .await
});

scene!(picture_mix, |width, height| {
    picture(
        width,
        height,
        change("c1", "mixer", "Bass volume 0.0 dB → -3.0 dB, pan C → 13L, send A 0 → 30 %", json!({
            "track":{"name":"Bass","color":"#f59a3c"},
            "picture":{"kind":"mix",
                "before":{"volume":0.85,"volumeText":"0.0 dB","pan":0.0,"panText":"C","sends":[{"name":"A Reverb","level":0.0,"text":"-inf dB"},{"name":"B Delay","level":0.2,"text":"-20 dB"}]},
                "after":{"volume":0.7,"volumeText":"-3.0 dB","pan":-0.26,"panText":"13L","sends":[{"name":"A Reverb","level":0.3,"text":"-12 dB"},{"name":"B Delay","level":0.2,"text":"-20 dB"}],"mute":false}}})),
    )
    .await
});

scene!(picture_notes, |width, height| {
    let note = |start: f64, pitch: f64, duration: f64| json!([start, pitch, duration, 100.0]);
    picture(
        width,
        height,
        change(
            "c1",
            "clip",
            "Chords · 3 notes moved, 1 added",
            json!({
            "track":{"name":"Keys","color":"#5ec1f7"},
            "picture":{"kind":"notes","length":8.0,
                "before":[note(0.,60.,2.),note(2.,62.,2.),note(4.,64.,2.)],
                "after":[note(0.,60.,1.),note(1.,67.,1.),note(2.,65.,2.),note(4.,64.,4.)]}}),
        ),
    )
    .await
});

scene!(picture_clip, |width, height| {
    let note = |pitch: f64, start: f64, duration: f64| json!({"pitch":pitch,"start":start,"duration":duration,"velocity":100.0});
    picture(
        width,
        height,
        change("c1", "clip", "New MIDI clip “Chord” · 6 notes", json!({
            "track":{"name":"Keys","color":"#5ec1f7"},
            "clip":{"length":8.0,"notes":[note(60.,0.,2.),note(64.,0.,2.),note(67.,0.,2.),note(57.,2.,2.),note(60.,2.,2.),note(65.,4.,4.)]}})),
    )
    .await
});

scene!(picture_device_in_rack, |width, height| {
    picture(
        width,
        height,
        change("c1", "device", "Loaded Saturator into Audio Effect Rack › Chain 1", json!({
            "track":{"name":"4-Audio","color":"#e2b93b"},
            "devices":{"rack":"Audio Effect Rack","chains":[{"name":"Chain 1","devices":["Saturator","EQ Eight"]},{"name":"Chain 2","devices":["Utility"]}],"chain":0,"index":0}})),
    )
    .await
});

scene!(picture_device_on_track, |width, height| {
    picture(
        width,
        height,
        change(
            "c1",
            "device",
            "Loaded Compressor on Bass",
            json!({
            "track":{"name":"Bass","color":"#f59a3c"},
            "devices":{"devices":["Operator","EQ Eight","Compressor","Utility"],"index":2}}),
        ),
    )
    .await
});

scene!(picture_color, |width, height| {
    picture(
        width,
        height,
        change(
            "c1",
            "color",
            "Bass colour orange → blue",
            json!({
            "track":{"name":"Bass","color":"#5ec1f7"},
            "colors":{"from":"#f59a3c","to":"#5ec1f7"}}),
        ),
    )
    .await
});

scene!(picture_before_pictures, |width, height| {
    picture(width, height, change("c1", "mixer", "Bass volume down", json!({"from":0.85,"to":0.6,"range":[0.0,1.0]}))).await
});

scene!(history_states, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    let state = |id: &str, family: &str, title: &str, state: &str, extra: Value| {
        let mut change = change(id, family, title, extra);
        change["change"]["state"] = json!(state);
        change
    };
    for change in [
        state("c1", "tempo", "Tempo 120 → 124 BPM", "applied", json!({"from":120.0,"to":124.0})),
        state(
            "c2",
            "mixer",
            "Bass width 100% → 140%, with the low end kept mono below 120 Hz so the sub stays centred",
            "applied",
            json!({"track":{"name":"Bass","color":"#f59a3c"}}),
        ),
        state("c3", "rename", "Renamed 2-MIDI to Lead", "undone", json!({})),
        state("c4", "structure", "Added return track “Drum Bus”", "kept", json!({"note":"Live's own undo can take it back."})),
        state("c5", "device", "Loaded Glue Compressor on Drums", "unsure", json!({"note":"Live didn't confirm it."})),
        state("c6", "clip", "New MIDI clip “Hook”", "expired", json!({"note":"Live restarted since."})),
        state("c7", "parameter", "Tried 3 pads against the reference", "heard", json!({"score":82.0})),
        state("c8", "parameter", "Listened to the drop", "heard", json!({})),
    ] {
        h.emit(change);
    }
    finish_styled(h).await
});

scene!(history_kept, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(json!({"type":"remembered","scope":"set","note":{"id":"s1","text":"The Reese is the main bass","at":1}}));
    h.emit(json!({"type":"remembered","scope":"producer","note":{"id":"p1","text":"Prefers short, dark reverbs","at":1}}));
    h.emit(json!({"type":"recipe","action":"saved","name":"Drum bus","steps":3}));
    h.emit(json!({"type":"technique","action":"kept","technique":{"id":"t1","name":"Neuro from a Reese","fits":"gritty neuro basses","source":"Au5 · Neuro bass"}}));
    h.emit(change("c1", "tempo", "Tempo 120 → 124 BPM", json!({"from":120.0,"to":124.0})));
    finish(h).await
});

scene!(now_watching, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.type_text("watch me make a drum bus\r").await;
    h.wait_for_call("submit:watch me make a drum bus").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Go ahead: I'm watching the Set."}));
    h.emit(json!({"type":"watching","on":true}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":1500}));
    h.emit(json!({"type":"state","state":"idle"}));
    finish(h).await
});

scene!(now_action, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(json!({"type":"action","title":"Playing from bar 17","playing":true}));
    finish(h).await
});

scene!(history_after_reconnect, |width, height| {
    let c = Rc::new(Control::default());
    c.set("reconnect", json!(true));
    let h = open(width, height, c).await;
    h.emit(change("c1", "tempo", "Tempo 120 → 128 BPM", json!({"from":120.0,"to":128.0})));
    h.type_text("/reconnect\r").await;
    h.wait_for_call("reconnect").await;
    h.wait_for("no undo").await;
    finish(h).await
});

// SetAsk, the panels, catch-up and resumed conversations.

fn conversations(c: &Control) {
    let now = now_ms();
    *c.conversations.borrow_mut() = serde_json::from_value(json!([
        {"id":"now001","savedAt":now,"first":"add a hi-hat groove","turns":2,"current":true,"place":"a"},
        {"id":"old001","savedAt":now-2*HOUR,"first":"make the bass wider","turns":5,"current":false,"place":"a"}
    ]))
    .unwrap();
    c.set("going", json!("now001"));
    c.set("all", json!([
        {"id":"now001","savedAt":now,"first":"add a hi-hat groove","turns":2,"current":true,"place":"a","set":"Night Drive"},
        {"id":"old001","savedAt":now-2*HOUR,"first":"make the bass wider","turns":5,"current":false,"place":"a","set":"Night Drive"},
        {"id":"far001","savedAt":now-3*HOUR,"first":"sketch a bassline","turns":4,"current":true,"place":"b","set":"Morning Run","pinned":true,"name":"Bass ideas"},
        {"id":"far002","savedAt":now-26*HOUR,"first":"sketch some chords","turns":1,"current":false,"place":"unsaved"}
    ]));
    c.set("lines", json!([{"role":"user","text":"sketch a bassline"},{"role":"assistant","text":"Wrote an eight-bar bassline on Bass."}]));
}

scene!(set_ask_other_set, |width, height| {
    let c = Rc::new(Control::default());
    conversations(&c);
    let h = open(width, height, c).await;
    h.emit(json!({"type":"set-asked","ask":{"id":"far001","title":"Bass ideas","set":"Morning Run","open":"Night Drive","daw":"Live","noun":"Set"}}));
    h.wait_for("Carry it on here, as a copy").await;
    finish(h).await
});

scene!(set_ask_unsaved, |width, height| {
    let c = Rc::new(Control::default());
    conversations(&c);
    let h = open(width, height, c).await;
    h.emit(json!({"type":"set-asked","ask":{"id":"far002","title":"","daw":"Live","noun":"Set"}}));
    h.wait_for("Carry it on here, as a copy").await;
    finish(h).await
});

scene!(panel_conversations_this_set, |width, height| {
    let c = Rc::new(Control::default());
    conversations(&c);
    let h = open(width, height, c).await;
    h.type_text("/conversations\r").await;
    h.wait_for("This Set · All Sets").await;
    finish(h).await
});

scene!(panel_conversations_all_sets, |width, height| {
    let c = Rc::new(Control::default());
    conversations(&c);
    let h = open(width, height, c).await;
    h.type_text("/resume\r").await;
    h.wait_for("This Set · All Sets").await;
    h.type_text("\t").await;
    h.wait_for("Morning Run").await;
    finish(h).await
});

scene!(panel_conversation_actions, |width, height| {
    let c = Rc::new(Control::default());
    conversations(&c);
    let h = open(width, height, c).await;
    h.type_text("/resume\r").await;
    h.wait_for("This Set · All Sets").await;
    h.type_text("\t").await;
    h.type_text("bassline").await;
    h.type_text("\x06").await;
    h.wait_for("Go back to it").await;
    finish(h).await
});

fn memory(c: &Control, saved: bool) {
    let now = now_ms();
    *c.memory.borrow_mut() = Some(
        serde_json::from_value(json!({
            "producer":[{"id":"p1","text":"Prefers short, dark reverbs","at":now-2*HOUR},{"id":"p2","text":"Mixes at -14 LUFS","at":now-26*HOUR,"pinned":true}],
            "set":[{"id":"s1","text":"The Reese is the main bass","at":now-HOUR}],
            "setName":"Night Drive","saved":saved}))
        .unwrap(),
    );
    c.set("techniques", json!([{"id":"t1","name":"Neuro from a Reese","fits":"gritty, moving neuro basses","source":"Au5 · Neuro bass"}]));
    c.set("recipes", json!([{"name":"Drum bus","about":"a return with glue compression","params":[],"steps":3,"used":2,"lastUsed":now-26*HOUR,"created":1}]));
    c.set("taste", json!([{"id":"chain-vocal","line":"Vocals: EQ Eight → Compressor → Reverb (on 4 of 4 vocal tracks)"},{"id":"tempo","line":"Tempo: usually 124–126 BPM"}]));
    *c.library.borrow_mut() =
        Some(serde_json::from_value(json!({"state":"ready","sounds":1020,"presets":40,"sets":3,"learnedAt":1})).unwrap());
}

scene!(panel_memory, |width, height| {
    let c = Rc::new(Control::default());
    memory(&c, true);
    let h = open(width, height, c).await;
    h.type_text("/memory\r").await;
    h.wait_for("What Kumi remembers").await;
    finish(h).await
});

scene!(panel_memory_unsaved_set, |width, height| {
    let c = Rc::new(Control::default());
    memory(&c, false);
    let h = open(width, height, c).await;
    h.type_text("/memory\r").await;
    h.wait_for("What Kumi remembers").await;
    finish(h).await
});

scene!(panel_memory_note, |width, height| {
    let c = Rc::new(Control::default());
    memory(&c, true);
    let h = open(width, height, c).await;
    h.type_text("/memory\r").await;
    h.wait_for("What Kumi remembers").await;
    h.type_text("\r").await;
    h.wait_for("Change the words").await;
    finish(h).await
});

scene!(panel_recipes, |width, height| {
    let c = Rc::new(Control::default());
    let now = now_ms();
    c.set("recipes", json!([
        {"name":"Drum bus","about":"Glue, saturation and a short room on a new return","params":[],"steps":3,"used":2,"lastUsed":now-26*HOUR,"created":1},
        {"name":"Resample twice","about":"OTT and Saturator, then Grain Delay","params":[{"name":"track","about":"the track to resample"}],"steps":6,"used":0,"created":2}
    ]));
    let h = open(width, height, c).await;
    h.type_text("/recipes\r").await;
    h.wait_for("Your recipes").await;
    finish(h).await
});

scene!(panel_jobs, |width, height| {
    let c = Rc::new(Control::default());
    let list = jobs();
    c.set("jobs", json!([list[0].clone(), list[2].clone(), list[4].clone()]));
    let h = open(width, height, c).await;
    for job in list {
        h.emit(job);
    }
    h.type_text("/jobs\r").await;
    h.wait_for("Manual: Grain Delay").await;
    finish_steady(h, &["Tape loop tutorial"]).await
});

scene!(notice_stop_without_live, |width, height| {
    let c = Rc::new(Control::default());
    c.stop.set(true);
    let h = open(width, height, c).await;
    lost(&h, "live", false);
    h.type_text("/stop\r").await;
    h.wait_for("nothing for Kumi to stop").await;
    finish(h).await
});

scene!(panel_recipe_run, |width, height| {
    let c = Rc::new(Control::default());
    c.set("recipes", json!([{"name":"Resample twice","about":"OTT and Saturator, then Grain Delay","params":[{"name":"track","about":"the track to resample"}],"steps":6,"used":0,"created":2}]));
    let h = open(width, height, c).await;
    h.type_text("/recipes\r").await;
    h.wait_for("Your recipes").await;
    h.type_text("\r").await;
    h.wait_for("Run it on…").await;
    finish(h).await
});

scene!(panel_technique_offered, |width, height| {
    let c = Rc::new(Control::default());
    c.set("techniques", json!([]));
    let h = open(width, height, c).await;
    h.type_text("recreate the bubbles from this tutorial\r").await;
    h.wait_for_call("submit:recreate the bubbles from this tutorial").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Built the bubble chain on Bubbles."}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
    h.emit(json!({"type":"technique","action":"offered","technique":{"id":"","name":"Liquid bubbles","fits":"bubbly FM effects"}}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.wait_for("1. Yes, keep it").await;
    finish(h).await
});

scene!(panel_btw, |width, height| {
    let c = Rc::new(Control::default());
    c.set("aside", json!("Erbe-Verb's tail runs **up to a minute**."));
    let h = open(width, height, c).await;
    h.type_text("/btw how long can its tail be?\r").await;
    h.wait_for("up to a minute").await;
    finish(h).await
});

scene!(panel_answers, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.type_text("sidechain the bass\r").await;
    h.wait_for_call("submit:sidechain the bass").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Which bass should duck under the kick?\n\n1. **Sub Bass**\n2. Reese\n3. Both"}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.wait_for("Your answer").await;
    finish(h).await
});

scene!(panel_commands, |width, height| {
    let c = Rc::new(Control::default());
    c.set("reconnect", json!(true));
    let h = open(width, height, c).await;
    h.type_text("/").await;
    h.wait_for("enter to choose").await;
    finish(h).await
});

scene!(panel_model, |width, height| {
    let fake = models::FakeModels::catalog();
    let h = Harness::with(width, height, Rc::new(Control::default()), |o| o.models = Some(fake));
    h.start().await;
    h.connect();
    h.type_text("/model\r").await;
    h.wait_for("enter to choose").await;
    finish(h).await
});

scene!(notice_help, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.type_text("/help\r").await;
    h.wait_for("ctrl+j").await;
    finish(h).await
});

scene!(notice_status, |width, height| {
    let c = Rc::new(Control::default());
    *c.library.borrow_mut() =
        Some(serde_json::from_value(json!({"state":"ready","sounds":1020,"presets":40,"sets":3,"learnedAt":1})).unwrap());
    c.set("now", json!({"id":"now001","title":"Bass ideas","kept":true,"touched":true}));
    let h = open(width, height, c).await;
    h.type_text("/status\r").await;
    h.wait_for("Conversation:").await;
    finish(h).await
});

fn willington_off() -> kumi::willington::WillingtonControl {
    let on = Rc::new(Cell::new(false));
    kumi::willington::WillingtonControl {
        on: {
            let on = on.clone();
            Rc::new(move || Some(on.get()))
        },
        set: Rc::new(move |value| {
            on.set(value);
            async move { Ok(if value { kumi::willington::TURNED_ON } else { kumi::willington::TURNED_OFF }) }.boxed_local()
        }),
    }
}

scene!(willington_off_at_start, |width, height| {
    let h = Harness::with(width, height, Rc::new(Control::default()), |o| o.willington = Some(willington_off()));
    h.start().await;
    h.connect();
    finish(h).await
});

scene!(catch_up_welcome, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":now_ms()-3*HOUR,
        "lines":["Tempo 120 → 124 BPM","Added track “Pad”","Renamed 2-MIDI to Lead"],"more":2}}));
    finish(h).await
});

scene!(catch_up_nothing_changed, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":now_ms()-3*HOUR,"lines":[],"more":0}}));
    finish(h).await
});

scene!(catch_up_notices, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(json!({"type":"notice","message":"Continuing your conversation from 3 hours ago. /new starts fresh."}));
    h.emit(json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":now_ms()-3*HOUR,"lines":["Tempo 120 → 124 BPM","Added track “Pad”"],"more":1}}));
    h.emit(json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":now_ms()-3*HOUR,"lines":[],"more":0}}));
    h.emit(json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":now_ms()-HOUR,"lines":["Muted Drums"],"more":0,"afterReconnect":true}}));
    finish(h).await
});

fn resumed(extra: Value) -> Value {
    let mut resumed = json!({"type":"resumed","savedAt":now_ms()-3*HOUR,
        "lines":[
            {"role":"user","text":"make the bass wider"},
            {"role":"assistant","text":"Widened it to 140%, keeping the sub mono.","tools":["live_discover","make_changes"]},
            {"role":"user","text":"and a bit darker"},
            {"role":"assistant","text":"Lowered the filter to 800 Hz."}],
        "changes":[
            {"id":"old001:c4","family":"mixer","title":"Bass width 100% → 140%","state":"expired","note":"From an earlier session, so Kumi can't undo it now.","at":1},
            {"id":"old001:c5","family":"parameter","title":"Operator · Filter Freq 1.20 kHz → 800 Hz","state":"expired","note":"From an earlier session, so Kumi can't undo it now.","at":2}]});
    for (key, value) in extra.as_object().unwrap() {
        resumed[key] = value.clone();
    }
    resumed
}

scene!(resumed_continuing, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(resumed(json!({})));
    finish(h).await
});

scene!(resumed_chosen, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.type_text("add a hi-hat groove\r").await;
    h.wait_for_call("submit:add a hi-hat groove").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Added a 16th-note hi-hat groove on Drums."}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":2100}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.emit(resumed(json!({"chosen":true})));
    finish(h).await
});

scene!(resumed_unreadable, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.emit(resumed(json!({"unreadable":true})));
    finish(h).await
});

// Each disconnect notice, while idle and while a turn runs, and what's said when Live is back.

/// The session's words for each cause, idle and working (core/session.rs).
const LOST: [(&str, &str, &str); 6] = [
    (
        "live",
        "Live closed. Kumi will pick up where you left off when it's back.",
        "Live closed while Kumi was working in it. If it crashed, your unsaved changes may be lost: Live offers to recover them when it opens. Kumi picks up when Live is back.",
    ),
    (
        "bridge",
        "Kumi's link to Live dropped. It's reconnecting, and will pick up where you left off.",
        "Kumi's link to Live dropped. It's reconnecting, and will pick up where you left off.",
    ),
    (
        "set",
        "Live is opening another Set; Kumi picks up once it's open.",
        "Live is opening another Set, so Kumi stopped what it was doing: nothing it was doing lands in the other Set. It picks up once the Set is open.",
    ),
    ("askedset", "Live is opening the Set; Kumi carries on once it's open.", "Live is opening the Set; Kumi carries on once it's open."),
    (
        "busy",
        "Live is busy and answering slowly; Kumi carries on once it answers.",
        "Live is busy and answering slowly, so Kumi is waiting for it: your request carries on once it answers.",
    ),
    (
        "none",
        "Kumi lost touch with Live. It will pick up where you left off when Live is back.",
        "Kumi lost touch with Live. It will pick up where you left off when Live is back.",
    ),
];
const STILL_MISSING: &str =
    "Kumi can't reach Live. Is it open, with AbletonMcpBridge chosen as a Control Surface (Settings → Link, Tempo & MIDI)?";

async fn working(h: &Harness) {
    h.type_text("make the kick punchier\r").await;
    h.wait_for_call("submit:make the kick punchier").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Raising the kick's transient first."}));
}

fn lost(h: &Harness, cause: &str, running: bool) {
    let (_, idle, busy) = LOST.iter().find(|(name, _, _)| *name == cause).unwrap();
    h.emit(json!({"type":"connection","state":"disconnected"}));
    h.emit(json!({"type":"notice","message":if running { busy } else { idle }}));
}

/// The stopped turn's end, as the session reports it after a cancel.
fn stopped(h: &Harness) {
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"cancelled"},"elapsedMs":1200}));
    h.emit(json!({"type":"state","state":"idle"}));
}

fn back(h: &Harness, set: &str, notices: &[&str]) {
    h.emit(json!({"type":"connection","state":"connected"}));
    h.emit(json!({"type":"observation","label":format!("Current open Set: {set} — Remote Script · real-live")}));
    for notice in notices {
        h.emit(json!({"type":"notice","message":notice}));
    }
}

macro_rules! disconnect {
    ($idle:ident, $running:ident, $cause:literal) => {
        scene!($idle, |width, height| {
            let h = open(width, height, Rc::new(Control::default())).await;
            lost(&h, $cause, false);
            finish(h).await
        });
        scene!($running, |width, height| {
            let h = open(width, height, Rc::new(Control::default())).await;
            working(&h).await;
            lost(&h, $cause, true);
            finish_steady(h, &["thinking"]).await
        });
    };
}
disconnect!(disconnect_live_idle, disconnect_live_running, "live");
disconnect!(disconnect_bridge_idle, disconnect_bridge_running, "bridge");
disconnect!(disconnect_set_idle, disconnect_set_running, "set");
disconnect!(disconnect_askedset_idle, disconnect_askedset_running, "askedset");
disconnect!(disconnect_busy_idle, disconnect_busy_running, "busy");
disconnect!(disconnect_none_idle, disconnect_none_running, "none");

scene!(disconnect_still_missing, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    lost(&h, "none", false);
    h.emit(json!({"type":"notice","message":STILL_MISSING}));
    finish(h).await
});

scene!(connecting, |width, height| {
    let h = Harness::new(width, height);
    h.start().await;
    h.emit(json!({"type":"connection","state":"connecting"}));
    finish(h).await
});

scene!(no_live, |width, height| {
    let h = Harness::new(width, height);
    h.start().await;
    finish(h).await
});

scene!(reconnect_back, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    lost(&h, "live", false);
    back(&h, "Night Drive", &["Live is back."]);
    finish(h).await
});

scene!(reconnect_resend, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    working(&h).await;
    lost(&h, "bridge", true);
    stopped(&h);
    back(&h, "Night Drive", &["Live is back. Your last request was stopped; press enter to send it again."]);
    h.emit(json!({"type":"resend","text":"make the kick punchier"}));
    finish(h).await
});

scene!(reconnect_after_a_crash, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    working(&h).await;
    lost(&h, "live", true);
    stopped(&h);
    back(&h, "Night Drive", &["Live is back. Your last request stopped when Live closed, which it may have caused: check the Set before you send it again (enter sends it)."]);
    h.emit(json!({"type":"resend","text":"make the kick punchier"}));
    finish(h).await
});

scene!(reconnect_other_set, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    lost(&h, "set", false);
    back(&h, "Morning Run", &["Live has the other Set open."]);
    finish(h).await
});

scene!(reconnect_busy, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    working(&h).await;
    lost(&h, "busy", true);
    back(&h, "Night Drive", &["Live is answering again."]);
    finish_steady(h, &["thinking"]).await
});

scene!(reconnect_busy_then_another_set, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    working(&h).await;
    lost(&h, "busy", true);
    back(
        &h,
        "Morning Run",
        &[
            "Live opened another Set while it was busy, so Kumi stopped what it was doing: nothing it was doing lands in the other Set.",
            "Live has the other Set open.",
        ],
    );
    stopped(&h);
    finish(h).await
});

// The jobs, goal, match and loop dashboards.

fn job(id: u32, state: &str, extra: Value) -> Value {
    let mut job = json!({"type":"job","id":id,"kind":"worker","title":"Burial vocal chops","state":state,"whenDone":"tell","startedAt":now_ms()-65_020,"steps":0,"sources":0});
    for (key, value) in extra.as_object().unwrap() {
        job[key] = value.clone();
    }
    job
}

fn jobs() -> Vec<Value> {
    let ended = now_ms();
    vec![
        job(1, "running", json!({"title":"Tape loop tutorial","kind":"tool","tool":"watch_video","doing":"watching a video","steps":1})),
        job(2, "running", json!({"doing":"reading a page","steps":2,"tool":"read_web"})),
        job(
            2,
            "done",
            json!({"line":"Burial pitches vocal snippets up and drowns them in reverb.","steps":5,"sources":3,"usage":{"inputTokens":11000.0,"outputTokens":1400.0,"cacheReadTokens":0.0,"cacheWriteTokens":0.0},"startedAt":ended-125_000,"endedAt":ended}),
        ),
        job(3, "running", json!({"title":"Manual: Grain Delay"})),
        job(
            3,
            "failed",
            json!({"title":"Manual: Grain Delay","line":"The manual couldn't be reached.","startedAt":ended-12_000,"endedAt":ended}),
        ),
    ]
}

scene!(dashboard_jobs, |width, height| {
    let c = Rc::new(Control::default());
    c.set("jobs", json!(true));
    let h = open(width, height, c).await;
    for job in jobs() {
        h.emit(job);
    }
    // Into the tabs, then on to JOBS.
    h.type_text("\x1b[Z").await;
    h.type_text("\x1b[Z").await;
    h.wait_for("watching a video").await;
    finish_steady(h, &["Tape loop tutorial"]).await
});

scene!(dashboard_job_detail, |width, height| {
    let c = Rc::new(Control::default());
    c.set("jobs", json!(true));
    let running = job(1, "running", json!({"doing":"reading a page","steps":2,"tool":"read_web"}));
    c.set(
        "job-details",
        json!([{"status":running,"asked":"Find how Burial chops vocals","steps":[
        {"tool":"search_web","text":"searched “Burial vocal chops”","state":"done"},
        {"tool":"read_web","text":"read a page","state":"failed"},
        {"tool":"search_web","text":"","state":"running"}]}]),
    );
    let h = open(width, height, c).await;
    h.emit(running);
    h.type_text("\x1b[Z").await;
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.wait_for("Asked: Find how Burial chops vocals").await;
    finish_steady(h, &["Burial vocal chops", "searching"]).await
});

scene!(dashboard_goal_searching, |width, height| {
    let c = Rc::new(Control::default());
    c.set("goal", json!(true));
    let h = open(width, height, c).await;
    h.type_text("/goal make my pad sound like ~/ref.wav\r").await;
    h.wait_for_call("goal:make my pad sound like ~/ref.wav").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"goal","state":"running","goal":"make my pad sound like ~/ref.wav","generation":12,"rendered":36,"trend":[58,58,61,64,64,70,71,71,74,76,76,81],"first":58,"best":{"label":"Collision","score":81},"leader":"Collision · Collision → Delay → Limiter","idea":"Tried a Collision with parallel delays.","elapsedMs":185_020,"candidates":3}));
    finish_steady(h, &["thinking"]).await
});

scene!(dashboard_goal_done, |width, height| {
    let c = Rc::new(Control::default());
    c.set("goal", json!(true));
    let h = open(width, height, c).await;
    h.type_text("/goal make my pad sound like ~/ref.wav\r").await;
    h.wait_for_call("goal:make my pad sound like ~/ref.wav").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"goal","state":"running","goal":"make my pad sound like ~/ref.wav","generation":12,"rendered":36,"trend":[58,81],"first":58,"best":{"label":"Collision","score":81},"elapsedMs":185_020,"candidates":3}));
    h.emit(json!({"type":"goal","state":"done","goal":"make my pad sound like ~/ref.wav","generation":13,"rendered":39,"trend":[58,61,64,70,74,81],"first":58,"best":{"label":"Collision","score":81},"elapsedMs":200_000,"candidates":3,"bestTrack":"Kumi · Goal best","why":"stopped"}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":200_000}));
    h.emit(json!({"type":"state","state":"idle"}));
    finish(h).await
});

fn objective(state: &str, extra: Value) -> Value {
    let mut objective = json!({"type":"objective","objective":"master this to -9 LUFS","state":state,"turns":1,"turnBudget":12,"elapsedMs":65_020,"budgetMs":3_600_000,"verdict":"continue","reason":"not met yet, by the model's own check","next":"raise the limiter","measured":false});
    for (key, value) in extra.as_object().unwrap() {
        objective[key] = value.clone();
    }
    objective
}

scene!(dashboard_objective, |width, height| {
    let c = Rc::new(Control::default());
    c.set("goal", json!(true));
    let h = open(width, height, c).await;
    h.type_text("/goal master this to -9 LUFS\r").await;
    h.wait_for_call("goal:master this to -9 LUFS").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(objective("running", json!({})));
    finish_steady(h, &["thinking"]).await
});

scene!(dashboard_loop, |width, height| {
    let c = Rc::new(Control::default());
    c.set("goal", json!(true));
    let h = open(width, height, c).await;
    h.type_text("/goal master this to -9 LUFS\r").await;
    h.wait_for_call("goal:master this to -9 LUFS").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(objective("running", json!({})));
    h.emit(json!({"type":"judged","round":1,"kind":"judged","heard":"the mix, bars 49–57","target":"Loudness","change":"Limiter gain +2 dB","rows":[],"kept":true,"why":"louder, punch kept","met":false,"listens":3,"elapsedMs":30000}));
    h.emit(json!({"type":"judged","round":2,"kind":"judged","heard":"the mix, bars 49–57","target":"True peak","change":"Limiter ceiling -1 → -3.5 dB","rows":[],"kept":false,"why":"it hurt punch (crest)","met":false,"listens":4,"elapsedMs":42000}));
    h.emit(json!({"type":"loop","state":"running","request":"master this to -9 LUFS","rounds":2,"kept":1,"reverted":1,"listens":7,"elapsedMs":42_020,"roundsLeft":14,"next":"True peak"}));
    finish_steady(h, &["thinking"]).await
});

scene!(dashboard_objective_met, |width, height| {
    let c = Rc::new(Control::default());
    c.set("goal", json!(true));
    let h = open(width, height, c).await;
    h.type_text("/goal master this to -9 LUFS\r").await;
    h.wait_for_call("goal:master this to -9 LUFS").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(objective("running", json!({})));
    h.emit(json!({"type":"loop","state":"running","request":"master this to -9 LUFS","rounds":1,"kept":1,"reverted":0,"listens":3,"elapsedMs":30_020,"roundsLeft":15,"next":"True peak"}));
    h.emit(json!({"type":"loop","state":"done","request":"master this to -9 LUFS","rounds":2,"kept":1,"reverted":1,"listens":5,"elapsedMs":61000,"roundsLeft":14,"stop":"met"}));
    h.emit(objective(
        "done",
        json!({"turns":2,"elapsedMs":90000,"verdict":"complete","reason":"the judge's checklist is met","measured":true,"next":""}),
    ));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":90000}));
    h.emit(json!({"type":"state","state":"idle"}));
    finish(h).await
});

scene!(dashboard_objective_blocked, |width, height| {
    let c = Rc::new(Control::default());
    c.set("goal", json!(true));
    let h = open(width, height, c).await;
    h.type_text("/goal master this to -9 LUFS\r").await;
    h.wait_for_call("goal:master this to -9 LUFS").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(objective("running", json!({})));
    h.emit(objective("paused", json!({"turns":3,"elapsedMs":120000,"verdict":"blocked","reason":"the Limiter is frozen","next":""})));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":120000}));
    h.emit(json!({"type":"state","state":"idle"}));
    finish(h).await
});

scene!(dashboard_matching, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.type_text("match the reference's low end\r").await;
    h.wait_for_call("submit:match the reference's low end").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"match","state":"running","check":3,"first":58,"best":{"label":"Low shelf +2 dB","score":74},"elapsedMs":42_020,"roundsLeft":5}));
    finish_steady(h, &["thinking"]).await
});

scene!(dashboard_matching_done, |width, height| {
    let h = open(width, height, Rc::new(Control::default())).await;
    h.type_text("match the reference's low end\r").await;
    h.wait_for_call("submit:match the reference's low end").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"match","state":"running","check":3,"first":58,"best":{"label":"Low shelf +2 dB","score":74},"elapsedMs":42_020,"roundsLeft":5}));
    h.emit(json!({"type":"match","state":"done","check":6,"first":58,"best":{"label":"Low shelf +3 dB","score":81},"elapsedMs":65_000,"roundsLeft":0,"stop":"reached"}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":65_000}));
    h.emit(json!({"type":"state","state":"idle"}));
    finish(h).await
});
