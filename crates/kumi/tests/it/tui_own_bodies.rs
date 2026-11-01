//! FOCUS draws REAPER's and Renoise's views as the producer moves between them: REAPER's MIDI editor shows the take's
//! notes and controller lanes, its arrange view each track's items, a track's routing its sends, an FX chain (a take's
//! too) its tree; Renoise's pattern and phrase editors show the lines around the cursor, its matrix the sequence, an
//! instrument its keyzones, the DSP chain its Doofers and Splitters. NOW and HISTORY draw each family of their changes.
#[path = "../support/models.rs"]
mod models;
#[path = "../support/tui_app.rs"]
mod support;
use kumi::daw::work_with;
use kumi::tui::{pictures::Graphics, style::ColorDepth};
use kumi_agent::core::contracts::*;
use kumi_daw_own::pictures as own_pictures;
use kumi_daw_reaper as reaper;
use kumi_daw_renoise as renoise;
use kumi_types::contracts::*;
use kumi_types::daw::DawId;
use serde_json::{json, Value};
use std::rc::Rc;
use support::*;
/// Waits (a little) for FOCUS to draw what a body read brings.
async fn shows(h: &Harness, text: &str) {
    for _ in 0..200 {
        if has(&h.screen(), text) {
            return;
        }
        delay(5).await;
    }
    h.has(text);
}
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

case!(reapers_midi_editor_arrange_view_and_routing_draw_in_focus, async {
    work_with(DawId::Reaper);
    let c = Rc::new(Control::default());
    *c.clip.borrow_mut() = Some(
        serde_json::from_value(json!({"slotRef":"r:take:{T1}","name":"Bass take","length":16.0,"notes":[
            {"pitch":36,"start":0.0,"duration":1.0,"velocity":100},{"pitch":43,"start":2.0,"duration":0.5,"velocity":90,"selected":true}
        ]}))
        .unwrap(),
    );
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let track = json!({"name":"Bass","color":"#3c7cf5","kind":"midi"});
    let midi = json!({"track":track,"trackRef":"r:track:{B}","mainView":{"kind":"timeline","label":"Arrange"},"editorView":{"kind":"note-editor","label":"MIDI editor"},"item":{"kind":"take","itemRef":"r:take:{T1}","name":"Bass take"}});
    h.emit(json!({"type":"focus","focus":midi}));
    shows(&h, "FOCUS · MIDI").await;
    h.has("Bass take");
    h.has("4 bars · 2 notes · 1 selected");
    assert!(h.calls().contains(&"clip:r:take:{T1}".to_string()), "{:?}", h.calls());

    // The routing window: the track's sends, with their levels and taps.
    *c.own.borrow_mut() = Some(FocusBody::Routing(RoutingView {
        track_ref: "r:track:{B}".into(),
        name: Some("Bass".into()),
        routes: vec![
            Route {
                kind: RouteKind::Send,
                other: "Reverb Bus".into(),
                volume_db: Some(-12.04),
                muted: false,
                tap: Some("post-fader".into()),
            },
            Route { kind: RouteKind::Receive, other: "Kick".into(), volume_db: None, muted: false, tap: Some("pre-fader".into()) },
        ],
        parent_send: Some(true),
        midi_out: Some("Launchpad · channel 2".into()),
    }));
    let routing = json!({"track":track,"trackRef":"r:track:{B}","mainView":{"kind":"timeline","label":"Arrange"},"editorView":{"kind":"routing","label":"Routing"}});
    h.emit(json!({"type":"focus","focus":routing}));
    shows(&h, "FOCUS · Routing").await;
    h.has("→ Reverb Bus · −12 dB · post-fader");
    h.has("← Kick · pre-fader");
    h.has("feeds its parent");
    h.has("⇥ MIDI to Launchpad · channel 2");

    // The arrange view: each track's items as bars on the timeline.
    *c.arrangement.borrow_mut() = Some(
        serde_json::from_value(
            json!({"length":64.0,"position":8.0,"playing":true,"locators":[{"name":"Verse","position":0.0}],"beatsPerBar":4.0,"track_count":200,"lanes":[
                {"name":"Drums","depth":0,"items":[{"start":0.0,"length":32.0,"name":"Beat"}]},
                {"name":"Bass","depth":1,"items":[{"start":16.0,"length":16.0,"name":"Bass take","selected":true}]}
            ]}),
        )
        .unwrap(),
    );
    let arrange = json!({"track":track,"trackRef":"r:track:{B}","mainView":{"kind":"timeline","label":"Arrange"}});
    h.emit(json!({"type":"focus","focus":arrange}));
    shows(&h, "FOCUS · Arrange").await;
    h.has("Drums");
    h.has("  Bass");
    h.has("▬▬");
    h.has("198 more tracks below");

    // A page of lanes from track 101 of 200, two tracks named Bass: the selected one (by its ref) is centred, and the
    // rest are counted above and below.
    let lane = |i: usize| {
        let name = if i == 1 || i == 8 { "Bass".to_string() } else { format!("T{i}") };
        json!({"name": name, "track_ref": format!("r:track:{{{i}}}"), "depth": 0, "items": []})
    };
    *c.arrangement.borrow_mut() = Some(
        serde_json::from_value(json!({"length":64.0,"position":8.0,"playing":false,"locators":[],"track_count":200,"lanes_from":100,
            "lanes":(0..10).map(lane).collect::<Vec<_>>()}))
        .unwrap(),
    );
    let arrange = json!({"track":{"name":"Bass"},"trackRef":"r:track:{8}","mainView":{"kind":"timeline","label":"Arrange"}});
    h.emit(json!({"type":"focus","focus":arrange}));
    shows(&h, "102 more tracks above · 90 below").await;

    // What REAPER doesn't draw, FOCUS doesn't either: a hidden track, and the tracks of a folder collapsed all the way
    // (marked ▸); they're counted, not drawn.
    *c.arrangement.borrow_mut() = Some(
        serde_json::from_value(json!({"length":64.0,"position":0.0,"playing":false,"locators":[],"track_count":5,"lanes":[
            {"name":"Drums","depth":0,"collapsed":true,"items":[]},
            {"name":"Kick","depth":1,"hidden":true,"items":[]},
            {"name":"Guide","depth":0,"hidden":true,"items":[]},
            {"name":"Keys","depth":0,"items":[]},
            {"name":"Vox","depth":0,"items":[]}
        ]}))
        .unwrap(),
    );
    let arrange = json!({"track":{"name":"Vox"},"trackRef":"r:track:{V}","mainView":{"kind":"timeline","label":"Arrange"}});
    h.emit(json!({"type":"focus","focus":arrange}));
    shows(&h, "2 hidden").await;
    h.has("▸ Drums");
    h.has("Keys");
    let screen = h.screen().join("\n");
    assert!(!screen.contains("Kick") && !screen.contains("Guide"), "{screen}");
    h.close().await;
});

case!(renoises_pattern_editor_matrix_and_keyzones_draw_in_focus, async {
    work_with(DawId::Renoise);
    let c = Rc::new(Control::default());
    let grid = TrackerGrid {
        item_ref: "3:pattern:2".into(),
        name: "Drop".into(),
        lines: 64,
        from: 0,
        cursor: Some(1),
        playing: Some(0),
        lpb: Some(4.),
        heading: None,
        facts: vec![],
        tracks: vec![
            TrackerTrack {
                name: "Kick".into(),
                color: None,
                cells: vec!["C-4 01 .. .. ..".into(), "--- .. .. .. ..".into(), "C-4 01 40 .. .. 0A10".into()],
            },
            TrackerTrack {
                name: "Bass".into(),
                color: Some("#3c7cf5".into()),
                cells: vec!["E-2 02 .. .. ..".into(), "OFF .. .. .. ..".into(), "--- .. .. .. ..".into()],
            },
        ],
    };
    *c.own.borrow_mut() = Some(FocusBody::Tracker(grid));
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let track = json!({"name":"Bass","kind":"sequencer"});
    let pattern = json!({"track":track,"trackRef":"3:track:2","mainView":{"kind":"pattern-editor","label":"Pattern Editor"},"item":{"kind":"pattern-track","itemRef":"3:pattern:2","name":"Drop"}});
    h.emit(json!({"type":"focus","focus":pattern}));
    shows(&h, "FOCUS · Pattern").await;
    h.has("Drop · 64 lines");
    h.has("E-2 02 .. .. ..");
    h.has("OFF .. .. .. ..");
    // The focused track comes first: in a narrow pane, it's the one that fits.
    let lines = h.screen();
    let first = lines.iter().position(|line| line.contains("E-2 02")).expect("the focused track's lines");
    assert!(lines.iter().take(first).any(|line| line.trim_end().ends_with("Bass") || line.contains(" Bass ")), "{}", lines.join("\n"));
    // Track names in their colours (review U12): the last Bass on screen is the pattern's column, under FOCUS's header.
    assert_eq!(h.styles_of("Bass").last().and_then(|style| style.split('|').next()), Some("60,124,245"));

    // The pattern matrix: each slot's pattern, then each track's part of it (review U9: not the pattern's number in every
    // column): filled, empty, muted, an alias of another pattern's; sections between, the current slot's pattern named.
    *c.own.borrow_mut() = Some(FocusBody::Matrix(SequenceMatrix {
        tracks: vec!["Kick".into(), "ベース".into(), "Bass".into()],
        slots: vec![
            SequenceSlot { pattern: 1, section: Some("Intro".into()), empty: vec![2], ..Default::default() },
            SequenceSlot {
                pattern: 2,
                muted: vec![1],
                aliases: vec![(2, 1)],
                colors: vec![(0, "#e04040".into())],
                name: Some("Drop".into()),
                ..Default::default()
            },
        ],
        current: Some(1),
        // A long sequence comes as the slots around the current one: they keep their numbers.
        first: 40,
    }));
    let matrix = json!({"track":track,"trackRef":"3:track:2","mainView":{"kind":"sequencer","label":"Pattern Matrix"},"section":{"kind":"sequence-slot","index":1.0}});
    h.emit(json!({"type":"focus","focus":matrix}));
    shows(&h, "FOCUS · Pattern Matrix").await;
    h.has("— Intro");
    h.has(" 41 01  ███ ███ ·");
    h.has(" 42 02  ███ ░░░ →01 Drop");
    // Columns by width: a wide name takes the cells it needs, and the next column stays in place.
    h.has("Kic ベ  Bas");

    // An instrument's keyzones: a map of them, keys across and velocities up, then each sample's notes, in Renoise's names
    // (MIDI 60 is C-5's octave); its phrases with their keys.
    *c.own.borrow_mut() = Some(FocusBody::Keyzones(KeyzoneMap {
        item_ref: "3:instrument:1".into(),
        name: "Drumkit".into(),
        zones: vec![
            Keyzone { sample: "Kick".into(), notes: (60, 60), velocities: (0, 127), base: 60, ..Default::default() },
            Keyzone { sample: "Snare".into(), notes: (62, 63), velocities: (64, 127), base: 62, ..Default::default() },
            Keyzone { sample: "Ghost".into(), notes: (63, 63), velocities: (0, 127), base: 63, note_off: true, ..Default::default() },
        ],
        macros: vec![("Drive".into(), 0.4)],
        phrases: vec!["Fill".into(), "Roll".into()],
        overlap: Some("cycle".into()),
        phrase_playback: Some("keymap".into()),
        phrase_keys: vec![Some((48, 59)), None],
        ..Default::default()
    }));
    let instrument = json!({"track":track,"trackRef":"3:track:2","mainView":{"kind":"instrument","label":"Instrument"},"item":{"kind":"sample","itemRef":"3:instrument:1","name":"Drumkit"}});
    h.emit(json!({"type":"focus","focus":instrument}));
    shows(&h, "FOCUS · Keyzones").await;
    h.has("Drumkit · overlapping zones: cycle");
    // (an octave of keys from G#4: the Kick at C-5 and the Ghost at D#5 at every velocity; loud, the Snare's D-5, and D#5
    // where the Snare and the Ghost overlap; named as Renoise writes them, review U10)
    h.has("····A·B▒····");
    h.has("····A··C····");
    h.has("G#4      G-5");
    h.has("A C-5       Kick");
    h.has("B D-5–D#5   Snare · vel 64–127");
    h.has("C D#5       Ghost · note-off");
    h.has("3 zones · 1 macro · 2 phrases");
    h.has("macros: Drive 40%");
    h.has("phrases by keymap: Fill C-4–B-4 · R");
    h.close().await;
});

/// Renoise can't say which pane has the keyboard, so FOCUS follows what changes: the pattern under the DSP frame, the
/// DSP chain for another device on the same track, the matrix when the producer moves through the sequence (never
/// when playback does), and the pattern again on another track.
#[test]
fn renoises_open_panes_show_when_the_producer_works_in_them() {
    use kumi::tui::app::{touched_next, Touched};
    use kumi_agent::core::view::DawFocus;
    work_with(DawId::Renoise);
    let focus = |track: u32, device: &str, slot: f64, follows: bool| -> DawFocus {
        serde_json::from_value(json!({
            "track": {"name": format!("Track {track}")}, "trackRef": format!("3:track:{track}"), "device": device,
            "mainView": {"kind": "pattern-editor", "label": "Pattern Editor"},
            "editorView": {"kind": "device-chain", "label": "Track DSPs"},
            "panes": [{"kind": "sequencer", "label": "Pattern Matrix"}],
            "item": {"kind": "pattern-track", "itemRef": format!("3:pattern:{}", slot as u32 + 1)},
            "section": {"kind": "sequence-slot", "index": slot},
            "sectionFollows": follows,
        }))
        .unwrap()
    };
    let start = focus(1, "Mixer", 0., false);
    assert_eq!(touched_next(None, Some(&start), None), Some(Touched::Tracker), "the pattern, though the DSP frame is open");
    let device = focus(1, "Delay", 0., false);
    assert_eq!(touched_next(Some(&start), Some(&device), Some(Touched::Tracker)), Some(Touched::Tree), "another device, same track");
    let track = focus(2, "Mixer", 0., false);
    assert_eq!(touched_next(Some(&device), Some(&track), Some(Touched::Tree)), Some(Touched::Tracker), "another track is the tracker's");
    let slot = focus(2, "Mixer", 1., false);
    assert_eq!(touched_next(Some(&track), Some(&slot), Some(Touched::Tracker)), Some(Touched::Matrix), "moving through the sequence");
    assert_eq!(touched_next(Some(&slot), Some(&slot), Some(Touched::Matrix)), Some(Touched::Matrix), "nothing changed, nothing moves");
    let played = focus(2, "Mixer", 2., true);
    assert_eq!(
        touched_next(Some(&track), Some(&played), Some(Touched::Tracker)),
        Some(Touched::Tracker),
        "playback moved it, not the producer"
    );
    let closed = DawFocus { panes: None, ..focus(2, "Mixer", 1., false) };
    assert_eq!(touched_next(Some(&track), Some(&closed), Some(Touched::Tracker)), Some(Touched::Tracker), "no matrix open, the pattern");
}

// Renoise's lower frame shows the track DSPs most of the time: FOCUS still draws the pattern, asked for by its ref.
case!(renoises_pattern_draws_under_an_open_dsp_frame, async {
    work_with(DawId::Renoise);
    let c = Rc::new(Control::default());
    *c.own.borrow_mut() = Some(FocusBody::Tracker(TrackerGrid {
        item_ref: "55cef1:pattern:1".into(),
        name: "".into(),
        lines: 64,
        from: 0,
        cursor: Some(0),
        playing: None,
        lpb: Some(4.),
        heading: None,
        facts: vec![],
        tracks: vec![TrackerTrack { name: "Track 01".into(), color: None, cells: vec!["--- .. .. .. .. ....".into()] }],
    }));
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let focus = json!({"track":{"name":"Track 01","kind":"sequencer"},"trackRef":"55cef1:track:1","mainView":{"kind":"pattern-editor","label":"Pattern Editor"},"editorView":{"kind":"device-chain","label":"Track DSPs"},"item":{"kind":"pattern-track","itemRef":"55cef1:pattern:1"},"section":{"kind":"sequence-slot","index":0.0}});
    h.emit(json!({"type":"focus","focus":focus}));
    shows(&h, "FOCUS · Pattern").await;
    h.has("Pattern · 64 lines");
    assert!(h.calls().contains(&"own:Tracker { item_ref: Some(\"55cef1:pattern:1\") }".to_string()), "{:?}", h.calls());
    h.close().await;
});

// The DAW sends lines well around the cursor (16 above it): FOCUS draws the rows around the cursor, so line 40 of 64
// is on screen, not cut off below the first 16.
case!(renoises_cursor_line_stays_in_view_deep_in_a_pattern, async {
    work_with(DawId::Renoise);
    let c = Rc::new(Control::default());
    let mut cells: Vec<String> = (0..33).map(|_| "--- .. .. .. ..".to_string()).collect();
    cells[16] = "OFF 01 .. .. ..".into();
    *c.own.borrow_mut() = Some(FocusBody::Tracker(TrackerGrid {
        item_ref: "3:pattern:2".into(),
        name: "Drop".into(),
        lines: 64,
        from: 23,
        cursor: Some(39),
        playing: None,
        lpb: Some(4.),
        heading: None,
        facts: vec![],
        tracks: vec![TrackerTrack { name: "Bass".into(), color: None, cells }],
    }));
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let focus = json!({"track":{"name":"Bass","kind":"sequencer"},"trackRef":"3:track:2","mainView":{"kind":"pattern-editor","label":"Pattern Editor"},"item":{"kind":"pattern-track","itemRef":"3:pattern:2","name":"Drop"}});
    h.emit(json!({"type":"focus","focus":focus}));
    shows(&h, "FOCUS · Pattern").await;
    let lines = h.screen();
    // (the cursor's line as Renoise numbers it: 39, the 40th)
    let cursor = lines.iter().find(|line| line.contains(" 39 ")).unwrap_or_else(|| panic!("line 39 on screen:\n{}", lines.join("\n")));
    assert!(cursor.contains("OFF 01"), "{cursor}");
    h.close().await;
});

case!(renoises_pattern_fits_the_drawer_around_its_cursor, async {
    work_with(DawId::Renoise);
    let c = Rc::new(Control::default());
    let mut cells: Vec<String> = (0..33).map(|_| "--- .. .. .. ..".to_string()).collect();
    cells[16] = "OFF 01 .. .. ..".into();
    *c.own.borrow_mut() = Some(FocusBody::Tracker(TrackerGrid {
        item_ref: "3:pattern:2".into(),
        name: "Drop".into(),
        lines: 64,
        from: 23,
        cursor: Some(39),
        playing: None,
        lpb: Some(4.),
        heading: None,
        facts: vec![],
        tracks: vec![TrackerTrack { name: "Bass".into(), color: None, cells }],
    }));
    let h = Harness::with(80, 30, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let focus = json!({"track":{"name":"Bass","kind":"sequencer"},"trackRef":"3:track:2","mainView":{"kind":"pattern-editor","label":"Pattern Editor"},"item":{"kind":"pattern-track","itemRef":"3:pattern:2","name":"Drop"}});
    h.emit(json!({"type":"focus","focus":focus}));
    let strip = |lines: &[String]| lines.iter().position(|line| line.starts_with("  NOW   FOCUS")).expect("the drawer");
    let lines = h.screen();
    h.type_text(&click(&lines, strip(&lines), "FOCUS")).await;
    shows(&h, "FOCUS · Pattern").await;
    // The drawer's six rows hold the pattern's title, its tracks' names and the lines around the cursor, the next one
    // too.
    let lines = h.screen();
    let drawer = &lines[strip(&lines) + 1..strip(&lines) + 7];
    let cursor =
        drawer.iter().position(|line| line.contains(" 39 ")).unwrap_or_else(|| panic!("line 39 in the drawer:\n{}", lines.join("\n")));
    assert!(drawer[cursor].contains("OFF 01") && drawer.get(cursor + 1).is_some_and(|line| line.contains(" 40 ")), "{}", lines.join("\n"));
    h.close().await;
});

// ----- goldens (m4-tui): each new body and each family's picture, drawn from what the bridges send -----
//
// Whole screens kept as text in tests/goldens/tui/daw (trailing spaces trimmed): at 120×36 and 160×40, narrow (80×30),
// and in the plain mode (no colour, pictures in words) at 120×36. `KUMI_BLESS=1` writes them again. The bodies come from
// bridge-shaped answers through each DAW's own reading (`to_body`, `fill_focus`), the pictures through Kumi's
// (`kumi_daw_own::pictures`), so a golden moves when any part of the way does.

fn golden(name: &str, actual: &str) {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/tui/daw").join(name);
    assert!(!actual.contains('\r'), "{name} has a CR in it");
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&file)
        .unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()))
        .replace("\r\n", "\n");
    if actual != expected {
        let (want, got): (Vec<&str>, Vec<&str>) = (expected.lines().collect(), actual.lines().collect());
        let first = (0..want.len().max(got.len())).find(|&row| want.get(row) != got.get(row)).unwrap_or(0);
        panic!(
            "{} changed at line {}: check it, then run this test with KUMI_BLESS=1\n- {}\n+ {}",
            file.display(),
            first + 1,
            want.get(first).unwrap_or(&"(no line)"),
            got.get(first).unwrap_or(&"(no line)")
        );
    }
}

fn text(lines: &[String]) -> String {
    lines.iter().map(|line| line.trim_end_matches(' ')).collect::<Vec<_>>().join("\n") + "\n"
}

/// The sizes a body is kept at: the narrowest has the narrowest FOCUS (100 columns); the last is the plain mode.
const BODY_SHOTS: &[(i32, i32, bool)] = &[(120, 36, false), (160, 40, false), (100, 30, false), (120, 36, true)];
/// A picture's: also below 100 columns, where the drawer under the conversation opens HISTORY to it.
const PICTURE_SHOTS: &[(i32, i32, bool)] = &[(120, 36, false), (160, 40, false), (100, 30, false), (80, 30, false), (120, 36, true)];

/// One scene at each of `shots`: the app for `daw`, started and connected; `drive` sets it up and leaves the screen to keep.
macro_rules! scene {
    ($name:ident, $daw:expr, $shots:expr, |$h:ident| $drive:block) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new()
                .run_until(async {
                    work_with($daw);
                    for &(width, height, plain) in $shots {
                        let $h = Harness::with(width, height, Rc::new(Control::default()), |o| {
                            if plain {
                                o.color_depth = Some(ColorDepth::None);
                                o.graphics = Some(Graphics::Words);
                            }
                        });
                        $h.start().await;
                        $h.connect();
                        $drive
                        let lines = $h.screen();
                        $h.close().await;
                        let name = format!("{}.{width}x{height}{}", stringify!($name).replace('_', "-"), if plain { ".plain" } else { "" });
                        golden(&format!("{name}.txt"), &text(&lines));
                    }
                })
                .await;
        }
    };
}

/// A focus as the app gets it: the bridge's answer read by the DAW's integration.
fn focus_of(fill: fn(&Value) -> DawFocus, bridge: Value) -> Value {
    json!({"type": "focus", "focus": serde_json::to_value(fill(&bridge)).unwrap()})
}

/// A change as the session tells the app, its picture drawn by Kumi from the preview's and the apply's answers.
fn drawn(id: &str, family: &str, title: &str, picture: Option<ChangePicture>) -> Value {
    let picture = picture.and_then(ChangePicture::bounded).expect("a picture");
    json!({"type": "change", "change": {"id": id, "family": family, "title": title, "state": "applied", "at": 1, "picture": picture}})
}

/// The change lands (NOW draws it), then the keyboard goes into HISTORY and opens it.
async fn opened(h: &Harness, change: Value) {
    h.emit(change);
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
}

// FOCUS

scene!(renoise_phrase, DawId::Renoise, BODY_SHOTS, |h| {
    let body = json!({"epoch": "e1", "ref": "e1:phrase:2.1", "instrumentRef": "e1:instrument:2", "name": "Fill", "lines": 16, "from": 1,
        "lpb": 4, "keyTracking": "transpose", "looping": true, "cursor": 5,
        "tracks": [{"ref": "e1:phrase:2.1", "name": "Fill", "noteColumns": 2, "effectColumns": 1, "lines": [
            {"line": 1, "notes": [{"col": 1, "note": "C-4", "instrument": "00", "volume": "80"}], "effects": [{"col": 1, "number": "0S", "amount": "20"}]},
            {"line": 5, "notes": [{"col": 1, "note": "E-4", "instrument": "00"}, {"col": 2, "note": "G-4", "instrument": "01", "volume": "40"}]},
            {"line": 9, "notes": [{"col": 1, "note": "OFF"}]},
            {"line": 13, "notes": [{"col": 2, "note": "B-4", "instrument": "01", "delay": "80"}]}]}]});
    let request = BodyRequest::Tracker { item_ref: Some("e1:phrase:2.1".into()) };
    *h.control.own.borrow_mut() = (renoise::DAW.to_body)(&request, &body);
    let focus = json!({"view": "phrase", "selection": {"track": {"name": "Lead", "type": "sequencer"}, "trackRef": "e1:track:2", "phraseRef": "e1:phrase:2.1"}});
    h.emit(focus_of(renoise::DAW.fill_focus, focus));
    shows(&h, "FOCUS · Phrase").await;
});

scene!(renoise_instrument, DawId::Renoise, BODY_SHOTS, |h| {
    let zone = |name: &str, notes: [u8; 2], velocities: [u8; 2], more: Value| {
        let mut zone = json!({"sample": name, "noteRange": notes, "velocityRange": velocities, "baseNote": notes[0]});
        for (key, value) in more.as_object().unwrap() {
            zone[key] = value.clone();
        }
        zone
    };
    let body = json!({"ref": "e1:instrument:1", "name": "Kit", "kind": "sample", "overlap": "all", "phrasePlayback": "program", "phraseProgram": 1,
        "keyzones": [
            zone("Kick", [36, 36], [0, 127], json!({})),
            zone("Snare", [38, 38], [0, 127], json!({"muteGroup": 1})),
            zone("Hat closed", [42, 42], [0, 127], json!({"muteGroup": 2})),
            zone("Hat open", [46, 46], [0, 127], json!({"muteGroup": 2})),
            zone("Break 01", [48, 48], [0, 127], json!({"slice": true})),
        ],
        "macros": [{"name": "Drive", "value": 0.25}, {"name": "Tone", "value": 0.6}],
        "phrases": ["Groove", "Fill"], "phraseRanges": [null, [60, 71]], "modsets": 1, "chains": 2});
    let request = BodyRequest::Keyzones { item_ref: "e1:instrument:1".into() };
    *h.control.own.borrow_mut() = (renoise::DAW.to_body)(&request, &body);
    let focus = json!({"view": "instrument", "selection": {"track": {"name": "Drums", "type": "sequencer"}, "trackRef": "e1:track:1",
        "instrumentRef": "e1:instrument:1", "instrumentName": "Kit"}});
    h.emit(focus_of(renoise::DAW.fill_focus, focus));
    shows(&h, "FOCUS · Keyzones").await;
});

scene!(renoise_dsp, DawId::Renoise, BODY_SHOTS, |h| {
    // The track's DSP chain as the host answers it: a Doofer and a Splitter with their devices (the Splitter splits by frequency).
    let inner = |at: [i64; 2], name: &str, lane: Option<i64>, enabled: bool| {
        let mut node = json!({"at": at, "name": name, "enabled": enabled, "plugin": false, "container": false, "childNodes": []});
        if let Some(lane) = lane {
            node["lane"] = lane.into();
        }
        node
    };
    let body = json!({"epoch": "e1", "ref": "e1:track:2", "name": "Bass", "nodes": [
        {"ref": "e1:device:2.1", "name": "Mixer", "enabled": true, "plugin": false, "container": false, "childNodes": []},
        {"ref": "e1:device:2.2", "name": "Grit", "enabled": true, "plugin": false, "container": true,
            "childNodes": [inner([2, 1], "Distortion", None, true), inner([2, 2], "EQ 5", None, false)]},
        {"ref": "e1:device:2.3", "name": "Bands", "enabled": true, "plugin": false, "container": true, "lanes": ["Low", "High"],
            "childNodes": [inner([3, 1], "Compressor", Some(1), true), inner([3, 1], "Chorus 2", Some(2), true), inner([3, 2], "Reverb", Some(2), true)]},
        {"ref": "e1:device:2.4", "name": "Gainer", "enabled": false, "plugin": false, "container": false, "childNodes": []},
    ]});
    let request = BodyRequest::Tree { track_ref: "e1:track:2".into() };
    let Some(FocusBody::Tree(tree)) = (renoise::DAW.to_body)(&request, &body) else { panic!() };
    *h.control.tree.borrow_mut() = Some(tree);
    let focus = |device: &str, reference: &str| {
        json!({"view": "pattern", "dspVisible": true, "selection": {"track": {"name": "Bass", "type": "sequencer"}, "trackRef": "e1:track:2",
            "patternRef": "e1:pattern:1", "deviceName": device, "deviceRef": reference}})
    };
    h.emit(focus_of(renoise::DAW.fill_focus, focus("Mixer", "e1:device:2.1")));
    delay(10).await;
    // Another device on the same track: the DSP chain shows, the Splitter's lanes open.
    h.emit(focus_of(renoise::DAW.fill_focus, focus("Bands", "e1:device:2.3")));
    shows(&h, "FOCUS · DSP").await;
});

// (review U17) A pitch bend lane is centred on 0, as REAPER says it: the centre a line, bends above it taller and below it
// shorter; in words, from −8192 to 8191.
#[test]
fn a_pitch_lane_is_drawn_and_said_centred_on_0() {
    let body = json!({"name": "Bend", "length": 4.0, "notes": [], "ccLanes": [
        {"cc": "pitchbend", "points": [[0.0, 8192], [1.0, 12288], [2.0, 0], [3.0, 16383]]}]});
    let Some(FocusBody::Notes(view)) = (reaper::DAW.to_body)(&BodyRequest::Notes { item_ref: "r:take:{T}".into() }, &body) else {
        panic!()
    };
    let lane = &view.lanes[0];
    assert_eq!((lane.min, lane.max, lane.points[0][1], lane.points[2][1]), (-8192., 8191., 0., -8192.));
    assert_eq!(kumi::tui::app::controller_line(lane, 4.0, 4), "─▇▁█");
}

scene!(reaper_midi_lanes, DawId::Reaper, BODY_SHOTS, |h| {
    let note = |pitch: i64, start: f64, length: f64, selected: bool| json!({"pitch": pitch, "start": start, "length": length, "velocity": 100, "selected": selected});
    let body = json!({"ref": "r:take:{T1}", "itemRef": "r:item:{I1}", "name": "Bass take", "length": 16.0, "beatsPerBar": 4.0, "noteCount": 4,
        "notes": [note(36, 0.0, 1.0, false), note(43, 2.0, 0.5, true), note(36, 4.0, 1.0, false), note(41, 6.0, 2.0, false)],
        "ccLanes": [
            {"cc": 1, "total": 5, "points": [[0.0, 0], [4.0, 32], [8.0, 64], [12.0, 96], [15.5, 127]]},
            {"cc": "pitchbend", "total": 3, "points": [[2.0, 8192], [3.0, 12000], [3.5, 8192]]},
            {"cc": 11, "total": 2, "points": [[0.0, 100], [8.0, 70]]},
            {"cc": "pressure", "total": 1, "points": [[1.0, 40]]}]});
    let request = BodyRequest::Notes { item_ref: "r:take:{T1}".into() };
    let Some(FocusBody::Notes(view)) = (reaper::DAW.to_body)(&request, &body) else { panic!() };
    *h.control.clip.borrow_mut() = Some(view);
    let focus = json!({"view": "midi", "selection": {"track": {"name": "Bass", "color": "#3c7cf5", "type": "midi"}, "trackRef": "r:track:{B}",
        "takeRef": "r:take:{T1}", "itemRef": "r:item:{I1}", "itemName": "Bass take"}});
    h.emit(focus_of(reaper::DAW.fill_focus, focus));
    shows(&h, "FOCUS · MIDI").await;
});

/// An FX node as REAPER's bridge answers it.
fn fx_node(reference: &str, name: &str, more: Value) -> Value {
    let mut node = json!({"ref": reference, "name": name, "enabled": true, "offline": false, "preset": "", "plugin": true, "container": false, "parallel": false});
    for (key, value) in more.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

scene!(reaper_fx_chain, DawId::Reaper, BODY_SHOTS, |h| {
    let body = json!({"ref": "r:track:{D}", "owner": "track", "ownerName": "Drums", "nodes": [
        fx_node("r:fx:{EQ}", "VST3: ReaEQ (Cockos)", json!({})),
        fx_node("r:fx:{BUS}", "Bus glue", json!({"container": true, "plugin": false, "childNodes": [
            fx_node("r:fx:{COMP}", "VST3: ReaComp (Cockos)", json!({})),
            fx_node("r:fx:{XC}", "VST3: ReaXcomp (Cockos)", json!({"parallel": true})),
            fx_node("r:fx:{LIM}", "JS: Limiter", json!({"enabled": false}))]})),
        fx_node("r:fx:{VERB}", "VST3: ReaVerbate (Cockos)", json!({"offline": true})),
    ], "inputNodes": [fx_node("r:fx:{GATE}", "VST3: ReaGate (Cockos)", json!({}))]});
    let request = BodyRequest::Tree { track_ref: "r:track:{D}".into() };
    let Some(FocusBody::Tree(tree)) = (reaper::DAW.to_body)(&request, &body) else { panic!() };
    *h.control.tree.borrow_mut() = Some(tree);
    let focus = json!({"view": "fx", "selection": {"track": {"name": "Drums", "color": "#ff5c5c", "type": "audio"}, "trackRef": "r:track:{D}",
        "fxRef": "r:fx:{XC}", "fxName": "VST3: ReaXcomp (Cockos)"}});
    h.emit(focus_of(reaper::DAW.fill_focus, focus));
    shows(&h, "FOCUS · FX").await;
});

scene!(reaper_take_fx, DawId::Reaper, BODY_SHOTS, |h| {
    let body = json!({"ref": "r:take:{K}", "owner": "take", "ownerName": "kick.wav", "nodes": [
        fx_node("r:fx:{TP}", "VST3: ReaPitch (Cockos)", json!({})),
        fx_node("r:fx:{TD}", "VST: ReaDelay (Cockos)", json!({}))], "inputNodes": []});
    let request = BodyRequest::Tree { track_ref: "r:take:{K}".into() };
    let Some(FocusBody::Tree(tree)) = (reaper::DAW.to_body)(&request, &body) else { panic!() };
    *h.control.tree.borrow_mut() = Some(tree);
    let focus = json!({"view": "fx", "selection": {"track": {"name": "Kick", "type": "audio"}, "trackRef": "r:track:{KT}",
        "fxRef": "r:fx:{TD}", "fxName": "VST: ReaDelay (Cockos)", "fxChainRef": "r:take:{K}"}});
    h.emit(focus_of(reaper::DAW.fill_focus, focus));
    shows(&h, "take “kick.wav”").await;
    assert!(h.calls().contains(&"tree:r:take:{K}".to_string()), "the take's chain is read, not the track's: {:?}", h.calls());
});

scene!(reaper_routing, DawId::Reaper, BODY_SHOTS, |h| {
    let route = |category: i64, other: &str, db: f64, mode: i64, mute: bool| json!({"category": category, "otherName": other, "volumeDb": db, "mode": mode, "mute": mute});
    let body = json!({"ref": "r:track:{B}", "name": "Bass", "parentSend": false, "midiOut": {"device": 2, "name": "Launchpad", "channel": 0},
        "routes": [route(0, "Reverb", -12.0, 0, false), route(0, "Sidechain", 0.0, 3, true), route(-1, "Kick", 0.0, 1, false), route(1, "output 3/4", -6.0, 0, false)]});
    let request = BodyRequest::Routing { track_ref: "r:track:{B}".into() };
    *h.control.own.borrow_mut() = (reaper::DAW.to_body)(&request, &body);
    let focus = json!({"view": "routing", "selection": {"track": {"name": "Bass", "color": "#3c7cf5", "type": "audio"}, "trackRef": "r:track:{B}",
        "routingTrackRef": "r:track:{B}"}});
    h.emit(focus_of(reaper::DAW.fill_focus, focus));
    shows(&h, "FOCUS · Routing").await;
});

scene!(reaper_mixer, DawId::Reaper, BODY_SHOTS, |h| {
    let strip = |name: &str, volume: f64, pan: f64, more: Value| {
        let mut row = json!({"ref": format!("r:track:{{{name}}}"), "name": name, "volumeDb": volume, "pan": pan, "mute": false, "solo": false,
            "selected": false, "peakDb": [volume - 9.0, volume - 11.0]});
        for (key, value) in more.as_object().unwrap() {
            row[key] = value.clone();
        }
        row
    };
    let body = json!({"strips": [
        strip("MASTER", 0.0, 0.0, json!({"master": true, "peakDb": [-0.5, -0.8]})),
        strip("Drums", -3.0, 0.0, json!({"color": "#ff5c5c", "depth": 0})),
        strip("Kick", -6.02, -0.25, json!({"selected": true, "arm": true, "depth": 1})),
        strip("Snare", -150.0, 0.3, json!({"mute": true, "depth": 1, "peakDb": [-150.0, -150.0]})),
        strip("Vocals with a long name", 1.5, 1.0, json!({"solo": true}))], "trackCount": 4, "from": 0});
    let request = BodyRequest::Mixer { track_ref: Some("r:track:{Kick}".into()) };
    *h.control.own.borrow_mut() = (reaper::DAW.to_body)(&request, &body);
    let focus = json!({"view": "mixer", "selection": {"track": {"name": "Kick", "type": "audio"}, "trackRef": "r:track:{Kick}"}});
    h.emit(focus_of(reaper::DAW.fill_focus, focus));
    shows(&h, "FOCUS · Mixer").await;
});

scene!(reaper_arrange_window, DawId::Reaper, BODY_SHOTS, |h| {
    // (#399 K35) The arrange view scrolled to bars 9–16 of 64: what it shows, the regions from start to end (the Verse
    // began before it), and a track's fixed lanes, a row each, the lanes that don't play faint.
    let item = |position: f64, length: f64, name: &str, more: Value| {
        let mut row = json!({"ref": format!("r:item:{{{name}}}"), "position": position, "length": length, "name": name, "source": "audio",
            "muted": false, "lane": 0, "lanePlays": null, "selected": false});
        for (key, value) in more.as_object().unwrap() {
            row[key] = value.clone();
        }
        row
    };
    let body = json!({"length": 256.0, "beatsPerBar": 4.0, "playhead": 40.0, "playing": true, "trackCount": 3, "from": 0,
        "window": {"from": 32.0, "to": 64.0}, "view": {"from": 32.0, "to": 64.0}, "loop": {"on": true, "start": 48.0, "end": 56.0},
        "markers": [{"name": "Drop", "position": 48.0}],
        "regions": [{"name": "Verse", "position": 16.0, "end": 48.0}, {"name": "Chorus", "position": 48.0, "end": 80.0}],
        "tracks": [
            {"ref": "r:track:{D}", "name": "Drums", "depth": 0, "items": [item(32.0, 32.0, "Beat", json!({}))]},
            {"ref": "r:track:{V}", "name": "Vox", "depth": 0, "fixedLanes": 3, "items": [
                item(32.0, 16.0, "Take 1", json!({"lanePlays": "no"})),
                item(36.0, 12.0, "Take 2", json!({"lane": 1, "lanePlays": "only", "selected": true})),
                item(40.0, 8.0, "Take 3", json!({"lane": 2, "lanePlays": "no"}))]},
            {"ref": "r:track:{B}", "name": "Bass", "depth": 0, "items": [item(32.0, 8.0, "Bass", json!({})), item(48.0, 16.0, "Bass 2", json!({"muted": true}))]}
        ]});
    let Some(FocusBody::Timeline(strip)) = (reaper::DAW.to_body)(&BodyRequest::Timeline, &body) else { panic!() };
    *h.control.arrangement.borrow_mut() = Some(strip);
    let focus = json!({"selection": {"track": {"name": "Vox", "type": "audio"}, "trackRef": "r:track:{V}"}});
    h.emit(focus_of(reaper::DAW.fill_focus, focus));
    shows(&h, "bars 9–16 of 64").await;
    h.has("[Chorus");
    h.has("lane 3");
});

scene!(renoise_mixer, DawId::Renoise, BODY_SHOTS, |h| {
    let strip = |n: u32, name: &str, kind: &str, volume: f64, pan: f64, mute: &str, solo: bool| json!({"ref": format!("e1:track:{n}"), "name": name, "type": kind, "volumeDb": volume, "pan": pan, "mute": mute, "solo": solo, "selected": n == 2});
    let body = json!({"strips": [
        strip(1, "Kick", "sequencer", 0.0, 0.0, "active", false),
        strip(2, "Bass", "sequencer", -6.0206, -0.5, "active", true),
        strip(3, "Pad", "sequencer", -12.0, 0.25, "off", false),
        strip(4, "Drums", "group", -1.5, 0.0, "muted", false),
        {"ref": "e1:track:5", "name": "Master", "type": "master", "volumeDb": 0.0, "pan": 0.0, "selected": false},
    ], "trackCount": 5, "from": 0, "levels": false});
    *h.control.own.borrow_mut() = (renoise::DAW.to_body)(&BodyRequest::Mixer { track_ref: None }, &body);
    let focus = json!({"view": "mixer", "selection": {"track": {"name": "Bass", "type": "sequencer"}, "trackRef": "e1:track:2"}});
    h.emit(focus_of(renoise::DAW.fill_focus, focus));
    shows(&h, "FOCUS · Mixer").await;
});

// NOW and HISTORY: one picture per family

/// The preview's and the apply's answers, as Kumi's pictures read them.
fn answers<'a>(preview: &'a Value, applied: &'a Value) -> own_pictures::Answers<'a> {
    own_pictures::Answers { preview, applied }
}

scene!(picture_lines, DawId::Renoise, PICTURE_SHOTS, |h| {
    let cell = |line: u32, note: &str, instrument: &str, volume: &str| json!({"line": line, "kind": "note", "col": 1, "note": note, "instrument": instrument, "volume": volume, "panning": "..", "delay": "..", "fx": "0000"});
    let effect =
        |line: u32, number: &str, amount: &str| json!({"line": line, "kind": "effect", "col": 1, "number": number, "amount": amount});
    let preview = json!({"prior": {"cells": [cell(1, "---", "..", ".."), effect(1, "00", "00"), cell(5, "C-4", "01", ".."), cell(9, "E-4", "01", "40"), cell(13, "---", "..", "..")]}});
    let applied = json!({"applied": {"cells": [cell(1, "C-4", "01", "80"), effect(1, "0A", "10"), cell(5, "C-4", "01", ".."), cell(9, "OFF", "..", ".."), cell(13, "G-4", "01", "60")]}});
    let picture = own_pictures::lines(&answers(&preview, &applied));
    opened(&h, drawn("c1", "clip", "Wrote 3 lines in “Drop” on “Bass”", picture)).await;
});

scene!(picture_curve, DawId::Reaper, PICTURE_SHOTS, |h| {
    let applied = json!({"display": {"curve": {"parameter": "Volume", "min": -60.0, "max": 12.0,
        "before": [[64.0, -6.0], [72.0, -6.0]], "after": [[64.0, -6.0], [66.0, -18.0], [68.0, 0.0], [70.0, -12.0], [72.0, -6.0]],
        "words": "-18 to 0 dB over bars 33–37"}}});
    let picture = own_pictures::curve(&answers(&json!({}), &applied));
    opened(&h, drawn("c1", "parameter", "Added 3 points to “Pad”'s Volume envelope", picture)).await;
});

scene!(picture_params, DawId::Reaper, PICTURE_SHOTS, |h| {
    let value = |name: &str, from: f64, to: f64, from_text: &str, to_text: &str| json!({"name": name, "from": from, "to": to, "min": 0.0, "max": 1.0, "fromText": from_text, "toText": to_text});
    let applied = json!({"display": {"values": [
        value("Freq-Band 1", 0.3, 0.12, "220.0 Hz", "120.0 Hz"), value("Gain-Band 1", 0.5, 0.38, "0.0 dB", "-3.5 dB"), value("BW-Band 2", 0.5, 0.75, "1.00", "3.00")]}});
    let picture = own_pictures::params(&answers(&json!({}), &applied));
    opened(&h, drawn("c1", "parameter", "ReaEQ 3 parameters on “Kick”", picture)).await;
});

// Renoise's one parameter, named with its range by the display; its automation, named too.
scene!(picture_param, DawId::Renoise, PICTURE_SHOTS, |h| {
    let before = json!({"value": 0.5, "valueString": "0.000 dB"});
    let preview = json!({"display": {"parameter": "Gain", "min": 0.0, "max": 1.4125, "before": before}});
    let applied = json!({"display": {"parameter": "Gain", "min": 0.0, "max": 1.4125, "before": before,
        "after": {"value": 0.25, "valueString": "-6.021 dB"}}});
    let picture = own_pictures::params(&answers(&preview, &applied));
    opened(&h, drawn("c1", "parameter", "Gainer on Bass Gain −6.021 dB", picture)).await;
});

scene!(picture_automation, DawId::Renoise, PICTURE_SHOTS, |h| {
    let point = |time: f64, value: f64| json!({"time": time, "value": value, "scaling": 0});
    let preview = json!({"prior": {"exists": false}});
    let applied = json!({"display": {"parameter": "Cutoff", "lines": 32},
        "applied": {"exists": true, "playmode": "linear", "points": [point(1., 0.1), point(9., 0.8), point(17., 0.4), point(25., 1.0)]}});
    let picture = own_pictures::curve(&answers(&preview, &applied));
    opened(&h, drawn("c1", "parameter", "Automation of Filter Cutoff on “Bass” in “Drop”: 4 points, linear", picture)).await;
});

scene!(picture_routes, DawId::Reaper, PICTURE_SHOTS, |h| {
    let applied = json!({"display": {"route": {"from": "Drums", "to": "Reverb", "change": "changed", "values": [
        {"name": "Volume", "from": -12.0, "to": -6.0, "min": -60.0, "max": 12.0, "fromText": "−12 dB", "toText": "−6 dB"},
        {"name": "Taps", "fromText": "post-fader", "toText": "pre-fader"}]}}});
    let picture = own_pictures::route(&answers(&json!({}), &applied));
    opened(&h, drawn("c1", "mixer", "Send “Drums” → “Reverb” −6 dB", picture)).await;
});

scene!(picture_zones, DawId::Renoise, PICTURE_SHOTS, |h| {
    let preview = json!({"display": {"before": {"noteRange": [48, 59], "velocityRange": [0, 127], "baseNote": 48}}});
    let applied = json!({"display": {"after": {"noteRange": [60, 71], "velocityRange": [64, 127], "baseNote": 60}}});
    let picture = own_pictures::zone(&answers(&preview, &applied), &kumi_types::daw::DawProfile::RENOISE);
    opened(&h, drawn("c1", "parameter", "“Snare” keyzone: notes 60-71, velocity 64-127, base note 60", picture)).await;
});

// Only the keys moved: the velocities aren't claimed (a band across the keys), and aren't said.
scene!(picture_zone_keys, DawId::Renoise, PICTURE_SHOTS, |h| {
    let preview = json!({"display": {"before": {"noteRange": [48, 59]}}});
    let applied = json!({"display": {"after": {"noteRange": [60, 71]}}});
    let picture = own_pictures::zone(&answers(&preview, &applied), &kumi_types::daw::DawProfile::RENOISE);
    opened(&h, drawn("c1", "parameter", "“Snare” keyzone: notes 60-71", picture)).await;
});

scene!(picture_items, DawId::Reaper, PICTURE_SHOTS, |h| {
    let item = |start: f64, length: f64, fade_out: f64, at: &str| json!({"name": "vox.wav", "start": start, "length": length, "fadeIn": 0.05, "fadeOut": fade_out, "muted": false, "at": at});
    let applied = json!({"display": {"items": {"before": [item(8.0, 4.0, 0.0, "bar 5")], "after": [item(10.0, 4.0, 0.5, "bar 6")]}}});
    let picture = own_pictures::items(&answers(&json!({}), &applied));
    opened(&h, drawn("c1", "clip", "Moved “vox.wav” to bar 6", picture)).await;
});

// (review U8) REAPER's notes changed in a take: the notes touched, before and after.
scene!(picture_notes, DawId::Reaper, PICTURE_SHOTS, |h| {
    let note = |pitch: u32, start: f64, length: f64| json!({"pitch": pitch, "start": start, "length": length, "velocity": 100});
    let preview = json!({"display": {"count": 3, "before": [note(36, 16.0, 1.0), note(36, 18.0, 0.5), note(43, 19.0, 0.5)],
        "after": [note(36, 16.0, 1.0), note(38, 18.0, 0.5), note(43, 19.5, 0.5)]}});
    let picture = own_pictures::midi(&answers(&preview, &json!({})));
    opened(&h, drawn("c1", "clip", "Changed 3 notes in “Bass take”", picture)).await;
});

scene!(picture_settings, DawId::Renoise, PICTURE_SHOTS, |h| {
    let preview = json!({"display": {"before": {"volumeDb": -6.0, "transpose": 0, "loop": "off", "nna": "cut"}}});
    let applied = json!({"display": {"after": {"volumeDb": -3.0, "transpose": 12, "loop": "forward", "nna": "cut"}}});
    let picture = own_pictures::settings(&answers(&preview, &applied));
    opened(&h, drawn("c1", "parameter", "“Pad” settings: volume −3 dB, transpose 12, loop forward", picture)).await;
});

// The transport in the DAW's words beside the tempo: REAPER's time signature where the playhead is (its tempo map's, no
// 4/4 assumed), Renoise's lines per beat.
case!(the_header_says_the_daws_time_beside_the_tempo, async {
    use kumi_daw_own::views as own_views;
    for (daw, transport, said) in [
        (DawId::Reaper, json!({"playing": true, "tempo": 96.0, "beat": 3.5, "beatsPerBar": 3.5, "timeSignature": [7, 8]}), "96 BPM · 7/8"),
        (DawId::Renoise, json!({"playing": true, "tempo": 125.0, "beat": 8.0, "beatsPerBar": 4, "lpb": 4, "tpl": 6}), "125 BPM · LPB 4"),
    ] {
        work_with(daw);
        let h = Harness::new(160, 40);
        h.start().await;
        h.connect();
        let read = own_views::transport(&json!({"transport": transport}), kumi_common::time::perf_now()).unwrap();
        h.emit(json!({"type": "transport", "transport": serde_json::to_value(read).unwrap()}));
        shows(&h, said).await;
        h.close().await;
    }
});

// Live's beat light is as it always was in odd meters: a 7/8 bar (3.5 beats) starting between beats isn't lit bright;
// REAPER's lights it (review L6).
case!(lives_beat_light_keeps_whole_beats_in_odd_meters_and_reapers_lights_the_bar_between_them, async {
    use kumi::tui::style::{palette, style_key, Style};
    use kumi_daw_own::views as own_views;
    for (daw, bright) in [(DawId::Ableton, false), (DawId::Reaper, true)] {
        work_with(daw);
        let h = Harness::new(160, 40);
        h.start().await;
        h.connect();
        // At 20 BPM a quarter of a beat is 750 ms: the light is read well inside it.
        let transport = json!({"playing": true, "tempo": 20.0, "beat": 3.5, "beatsPerBar": 3.5});
        let read = own_views::transport(&json!({"transport": transport}), kumi_common::time::perf_now()).unwrap();
        h.emit(json!({"type": "transport", "transport": serde_json::to_value(read).unwrap()}));
        shows(&h, "20 BPM").await;
        let light = h.style_before(" 20 BPM").expect("the light");
        let want = style_key(&Style::fg(if bright { palette::BEAT } else { palette::OFFBEAT }));
        // (its foreground: the header's background is the screen's)
        assert_eq!(light.split('|').next(), want.split('|').next(), "{daw:?}");
        h.close().await;
    }
    work_with(DawId::Ableton);
});

// An unnamed REAPER take in FOCUS is an "Untitled take", never Live's "Untitled clip" (review L9).
case!(an_unnamed_reaper_take_is_an_untitled_take, async {
    work_with(DawId::Reaper);
    let h = Harness::with(120, 36, Rc::new(Control::default()), |_| {});
    h.start().await;
    h.connect();
    let body = json!({"ref": "r:take:{T1}", "itemRef": "r:item:{I1}", "name": "", "length": 4.0, "beatsPerBar": 4.0, "noteCount": 1,
        "notes": [{"pitch": 36, "start": 0.0, "length": 1.0, "velocity": 100, "selected": false}], "ccLanes": []});
    let request = BodyRequest::Notes { item_ref: "r:take:{T1}".into() };
    let Some(FocusBody::Notes(view)) = (reaper::DAW.to_body)(&request, &body) else { panic!() };
    *h.control.clip.borrow_mut() = Some(view);
    let focus = json!({"view": "midi", "selection": {"track": {"name": "Bass", "type": "midi"}, "trackRef": "r:track:{B}",
        "takeRef": "r:take:{T1}", "itemRef": "r:item:{I1}"}});
    h.emit(focus_of(reaper::DAW.fill_focus, focus));
    shows(&h, "Untitled take").await;
    assert!(!h.screen().join("\n").contains("Untitled clip"));
    h.close().await;
    work_with(DawId::Ableton);
});

// (review K2) REAPER's routing window for a track that isn't the selected one: the focus names no track on purpose (the
// selected track's name isn't the window's), and FOCUS heads the routing with the name its body gives, never panics.
case!(reapers_routing_window_for_another_track_draws_under_its_own_name, async {
    work_with(DawId::Reaper);
    let c = Rc::new(Control::default());
    let body = json!({"ref": "r:track:{V}", "name": "Vocals", "parentSend": true,
        "routes": [{"category": 0, "otherName": "Plate", "volumeDb": -9.0, "mode": 0, "mute": false}]});
    *c.own.borrow_mut() = (reaper::DAW.to_body)(&BodyRequest::Routing { track_ref: "r:track:{V}".into() }, &body);
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let bridge = json!({"view": "routing", "selection": {"track": {"name": "Bass", "type": "audio"}, "trackRef": "r:track:{B}",
        "routingTrackRef": "r:track:{V}"}});
    let focus = (reaper::DAW.fill_focus)(&bridge);
    assert!(focus.track.is_none(), "the focus names no track for another track's routing window");
    h.emit(json!({"type": "focus", "focus": serde_json::to_value(focus).unwrap()}));
    shows(&h, "→ Plate · −9 dB · post-fader").await;
    h.has("Vocals");
    assert!(!has(&h.screen(), "Bass"), "not the selected track's name");
    h.close().await;
});

/// (review K10) REAPER's arrange comes back once its MIDI editor or an FX window loses the focus: the body follows the
/// editor going away, as it follows one coming.
#[test]
fn reapers_arrange_comes_back_after_the_midi_editor_or_an_fx_window() {
    use kumi::tui::app::{touched_next, Touched};
    work_with(DawId::Reaper);
    let focus = |view: &str| -> DawFocus {
        (reaper::DAW.fill_focus)(&json!({"view": view, "selection": {"track": {"name": "Bass", "type": "midi"}, "trackRef": "r:track:{B}",
            "takeRef": "r:take:{T}", "itemRef": "r:item:{I}", "fxRef": "r:fx:{F}", "fxName": "ReaEQ"}}))
    };
    let (arrange, midi, fx) = (focus("arrange"), focus("midi"), focus("fx"));
    assert_eq!(touched_next(None, Some(&arrange), None), Some(Touched::Timeline));
    let editing = touched_next(Some(&arrange), Some(&midi), Some(Touched::Timeline));
    assert_eq!(touched_next(Some(&midi), Some(&arrange), editing), Some(Touched::Timeline), "MIDI editor → arrange");
    let tree = touched_next(Some(&arrange), Some(&fx), Some(Touched::Timeline));
    assert_eq!(tree, Some(Touched::Tree));
    assert_eq!(touched_next(Some(&fx), Some(&arrange), tree), Some(Touched::Timeline), "FX window → arrange");
    assert_eq!(touched_next(Some(&arrange), Some(&arrange), Some(Touched::Timeline)), Some(Touched::Timeline), "nothing moved");
}

/// A pattern of Renoise's, one Bass line of it.
fn bass_pattern(cell: &str) -> FocusBody {
    FocusBody::Tracker(TrackerGrid {
        item_ref: "3:pattern:1".into(),
        name: "Drop".into(),
        lines: 64,
        cursor: Some(0),
        lpb: Some(4.),
        tracks: vec![TrackerTrack { name: "Bass".into(), color: None, cells: vec![cell.into()] }],
        ..Default::default()
    })
}
fn bass_pattern_focus() -> Value {
    json!({"type": "focus", "focus": {"track": {"name": "Bass", "kind": "sequencer"}, "trackRef": "3:track:1",
        "mainView": {"kind": "pattern-editor", "label": "Pattern Editor"}, "item": {"kind": "pattern-track", "itemRef": "3:pattern:1"}}})
}
// (review K18) A body of Renoise's own that reads empty (a refused read: Renoise while Kumi renders) comes back: the body
// shown stays through a refused read, and with none shown yet the next focus asks again.
case!(renoises_pattern_comes_back_after_a_read_that_brought_nothing, async {
    work_with(DawId::Renoise);
    let c = Rc::new(Control::default());
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    // Nothing the first time (refused): the next focus asks again at once (not only on the 1.5 s refresh), and it's drawn.
    *c.own.borrow_mut() = Some(bass_pattern("E-2 02 .. .. .."));
    c.own_refused.set(true);
    h.emit(bass_pattern_focus());
    delay(30).await;
    assert!(!has(&h.screen(), "E-2 02 .. .. .."));
    c.own_refused.set(false);
    h.emit(bass_pattern_focus());
    for _ in 0..30 {
        if has(&h.screen(), "E-2 02 .. .. ..") {
            break;
        }
        delay(10).await;
    }
    h.has("E-2 02 .. .. ..");
    // A refused read (here asked at once, as a reconnect asks) leaves the pattern drawn.
    c.own_refused.set(true);
    h.emit(json!({"type": "connection", "state": "connected"}));
    delay(60).await;
    h.has("E-2 02 .. .. ..");
    // And the next read that brings it is drawn.
    c.own_refused.set(false);
    *c.own.borrow_mut() = Some(bass_pattern("G-2 02 .. .. .."));
    h.emit(json!({"type": "connection", "state": "connected"}));
    shows(&h, "G-2 02 .. .. ..").await;
    // Away and back (another song may be open now): what was shown before isn't kept through a refused read.
    c.own_refused.set(true);
    h.emit(json!({"type": "connection", "state": "disconnected"}));
    h.emit(json!({"type": "connection", "state": "connected"}));
    delay(60).await;
    assert!(!has(&h.screen(), "G-2 02 .. .. .."), "{}", h.screen().join("\n"));
    h.close().await;
});
// (review K18) With nothing shown yet and no focus coming (the producer stays on the pattern while Kumi renders), the
// pane's timer still reads the pattern again, so it's drawn once Renoise answers.
case!(renoises_pattern_refused_while_nothing_is_shown_is_read_again_on_the_timer, async {
    tokio::time::pause();
    work_with(DawId::Renoise);
    let c = Rc::new(Control::default());
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    *c.own.borrow_mut() = Some(bass_pattern("E-2 02 .. .. .."));
    c.own_refused.set(true);
    h.emit(bass_pattern_focus());
    delay(5).await;
    assert!(!has(&h.screen(), "E-2 02 .. .. .."));
    c.own_refused.set(false);
    tokio::time::advance(std::time::Duration::from_millis(1600)).await;
    delay(5).await;
    h.has("E-2 02 .. .. ..");
    h.close().await;
});
// (review U4) REAPER's FOCUS counts bars as REAPER does, not in 4/4: by the meter the arrange body sends, and a take's
// bars by its time signature; the time selection is said and shaded.
case!(reapers_focus_counts_bars_in_the_projects_meter, async {
    work_with(DawId::Reaper);
    let c = Rc::new(Control::default());
    *c.clip.borrow_mut() = Some(
        serde_json::from_value(json!({"slotRef":"r:take:{T1}","name":"Waltz","length":12.0,"beatsPerBar":3.0,"notes":[
            {"pitch":36,"start":0.0,"duration":1.0,"velocity":100}
        ]}))
        .unwrap(),
    );
    *c.arrangement.borrow_mut() = Some(
        serde_json::from_value(json!({"length":31.0,"position":16.0,"playing":true,"locators":[],"beats_per_bar":3.0,
            "loop":{"start":6.0,"length":6.0,"enabled":true},"time_selection":[24.0,27.5],
            "meter":[{"position":0.0,"bar":1,"beat":0.0,"beats_per_bar":3.0},{"position":24.0,"bar":9,"beat":0.0,"beats_per_bar":3.5}],
            "lanes":[{"name":"Bass","depth":0,"items":[]}]}))
        .unwrap(),
    );
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let track = json!({"name":"Bass","color":"#3c7cf5","kind":"midi"});
    let midi = json!({"track":track,"trackRef":"r:track:{B}","mainView":{"kind":"timeline","label":"Arrange"},"editorView":{"kind":"note-editor","label":"MIDI editor"},"item":{"kind":"take","itemRef":"r:take:{T1}","name":"Waltz"}});
    h.emit(json!({"type":"focus","focus":midi}));
    shows(&h, "4 bars · 1 note").await;
    let arrange = json!({"track":track,"trackRef":"r:track:{B}","mainView":{"kind":"timeline","label":"Arrange"}});
    h.emit(json!({"type":"focus","focus":arrange}));
    shows(&h, "bar 6 · playing").await;
    h.has("loop 3–5 · selection 9–10 · 10 bars");
    h.close().await;
});
// (review U7) Renoise's pattern follows the cursor and the playing line between reads (every 1.5 s): the focus's line
// moves the cursor, the transport's the playing line; a line past those read, or playback back at an earlier line,
// reads the pattern again.
case!(renoises_pattern_follows_the_cursor_and_playback_between_reads, async {
    tokio::time::pause();
    work_with(DawId::Renoise);
    let c = Rc::new(Control::default());
    *c.own.borrow_mut() = Some(FocusBody::Tracker(TrackerGrid {
        item_ref: "3:pattern:1".into(),
        name: "Drop".into(),
        lines: 64,
        from: 0,
        cursor: Some(0),
        playing: Some(2),
        lpb: Some(4.),
        tracks: vec![TrackerTrack { name: "Bass".into(), color: None, cells: (0..33).map(|i| format!("C-4 {i:02} .. .. ..")).collect() }],
        ..Default::default()
    }));
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let focus = |line: usize| {
        json!({"type": "focus", "focus": {"track": {"name": "Bass", "kind": "sequencer"}, "trackRef": "3:track:1", "line": line,
            "mainView": {"kind": "pattern-editor", "label": "Pattern Editor"}, "item": {"kind": "pattern-track", "itemRef": "3:pattern:1"}}})
    };
    let transport = |line: usize| json!({"type": "transport", "transport": {"playing": true, "at": 0.0, "line": line}});
    let reads = |h: &Harness| h.calls().iter().filter(|call| call.starts_with("own:Tracker")).count();
    let [r, g, b] = kumi::tui::style::palette::RAISED;
    let raised =
        |h: &Harness, cells: &str| h.styles_before(cells, 1).iter().any(|style| style.split('|').nth(1) == Some(&format!("{r},{g},{b}")));
    let [r, g, b] = kumi::tui::style::palette::ACCENT;
    let playing =
        |h: &Harness, cells: &str| h.styles_before(cells, 4).iter().any(|style| style.split('|').next() == Some(&format!("{r},{g},{b}")));
    h.emit(focus(0));
    shows(&h, "C-4 00").await;
    // (playback starting reads the pattern again, once)
    h.emit(transport(2));
    delay(5).await;
    let read = reads(&h);
    assert!(raised(&h, "C-4 00 ") && playing(&h, "C-4 02 "), "{}", h.screen().join("\n"));
    h.emit(focus(6));
    h.emit(transport(5));
    delay(5).await;
    assert!(raised(&h, "C-4 06 ") && !raised(&h, "C-4 00 "), "the cursor moved:\n{}", h.screen().join("\n"));
    assert!(playing(&h, "C-4 05 ") && !playing(&h, "C-4 02 "), "the playing line moved:\n{}", h.screen().join("\n"));
    assert_eq!(reads(&h), read, "no read for that");
    // Playback back at line 1 (the next pattern, or a loop): read again.
    h.emit(transport(1));
    delay(5).await;
    assert_eq!(reads(&h), read + 1);
    // The cursor past the lines read: read again.
    h.emit(focus(40));
    delay(5).await;
    assert_eq!(reads(&h), read + 2);
    h.close().await;
});
// (review K35) Tab goes into the chain FOCUS shows, a take's FX too; a device the DAW names no ref for (in a Splitter's lane)
// is pinned with its container's ref, which the tools read, and the row pinned is the one marked.
case!(tab_enters_a_takes_fx_and_a_device_without_a_ref_pins_its_container, async {
    work_with(DawId::Reaper);
    let c = Rc::new(Control::default());
    let body = json!({"ref": "r:take:{K}", "owner": "take", "ownerName": "kick.wav", "nodes": [
        fx_node("r:fx:{TP}", "VST3: ReaPitch (Cockos)", json!({})), fx_node("r:fx:{TD}", "VST: ReaDelay (Cockos)", json!({}))]});
    let Some(FocusBody::Tree(tree)) = (reaper::DAW.to_body)(&BodyRequest::Tree { track_ref: "r:take:{K}".into() }, &body) else { panic!() };
    *c.tree.borrow_mut() = Some(tree);
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let focus = json!({"view": "fx", "selection": {"track": {"name": "Kick", "type": "audio"}, "trackRef": "r:track:{KT}",
        "fxRef": "r:fx:{TD}", "fxName": "VST: ReaDelay (Cockos)", "fxChainRef": "r:take:{K}"}});
    h.emit(focus_of(reaper::DAW.fill_focus, focus));
    shows(&h, "take “kick.wav”").await;
    h.type_text("\t").await;
    assert!(h.has_selection_highlight(), "into the take's chain:\n{}", h.screen().join("\n"));
    h.close().await;

    work_with(DawId::Renoise);
    let c = Rc::new(Control::default());
    let body = json!({"epoch": "e1", "ref": "e1:track:2", "name": "Bass", "nodes": [
        {"ref": "e1:device:2.3", "name": "Bands", "enabled": true, "plugin": false, "container": true, "lanes": ["Low"],
            "childNodes": [{"at": [3, 1], "name": "Compressor", "lane": 1, "enabled": true, "plugin": false, "container": false, "childNodes": []}]}]});
    let Some(FocusBody::Tree(tree)) = (renoise::DAW.to_body)(&BodyRequest::Tree { track_ref: "e1:track:2".into() }, &body) else {
        panic!()
    };
    *c.tree.borrow_mut() = Some(tree);
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let focus = |device: &str, reference: &str| {
        json!({"view": "pattern", "dspVisible": true, "selection": {"track": {"name": "Bass", "type": "sequencer"}, "trackRef": "e1:track:2",
            "patternRef": "e1:pattern:1", "deviceName": device, "deviceRef": reference}})
    };
    // (another device on the same track: the DSP chain shows)
    h.emit(focus_of(renoise::DAW.fill_focus, focus("Mixer", "e1:device:2.1")));
    delay(10).await;
    h.emit(focus_of(renoise::DAW.fill_focus, focus("Bands", "e1:device:2.3")));
    shows(&h, "FOCUS · DSP").await;
    // Into the tree (on Bands), down past the Low lane to the Compressor, and pin it.
    h.type_text("\t").await;
    h.type_text("\x1b[B").await;
    h.type_text("\x1b[B").await;
    h.type_text("\r").await;
    let lines = h.screen();
    let row = lines.iter().find(|line| line.contains("◇  Compressor")).unwrap_or_else(|| panic!("{}", lines.join("\n")));
    assert!(row.contains("pinned"), "the row pinned is marked:\n{}", lines.join("\n"));
    h.type_text("what is this\r").await;
    let pin = c.pins.borrow().last().cloned().flatten().expect("the pin went with the turn");
    assert_eq!((pin.r#ref.as_str(), pin.name.as_str()), ("e1:device:2.3", "Compressor"));
    h.close().await;
});
// (review K35) A big take says every note it has, not the count of the first ones the body lists.
case!(a_big_takes_note_count_is_the_takes, async {
    work_with(DawId::Reaper);
    let c = Rc::new(Control::default());
    let body = json!({"ref": "r:take:{T1}", "name": "Arp", "length": 16.0, "beatsPerBar": 4.0, "noteCount": 1200,
        "notes": [{"pitch": 60, "start": 0.0, "length": 0.25, "velocity": 100}, {"pitch": 64, "start": 0.25, "length": 0.25, "velocity": 100}]});
    let Some(FocusBody::Notes(view)) = (reaper::DAW.to_body)(&BodyRequest::Notes { item_ref: "r:take:{T1}".into() }, &body) else {
        panic!()
    };
    *c.clip.borrow_mut() = Some(view);
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let midi = json!({"track":{"name":"Keys","kind":"midi"},"trackRef":"r:track:{K}","mainView":{"kind":"timeline","label":"Arrange"},
        "editorView":{"kind":"note-editor","label":"MIDI editor"},"item":{"kind":"take","itemRef":"r:take:{T1}","name":"Arp"}});
    h.emit(json!({"type":"focus","focus":midi}));
    shows(&h, "4 bars · 1200 notes").await;
    h.close().await;
});
