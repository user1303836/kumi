//! Changes' pictures in HISTORY (a row opens to show it), NOW, the drawer's NOW and the narrow dock; images only where
//! the terminal shows them.
#[path = "../support/tui_app.rs"]
mod support;
use kumi::tui::pictures::Graphics;
use serde_json::{json, Value};
use std::rc::Rc;
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

fn eq_change(id: &str, gain: f64) -> Value {
    let band = |gain: f64| json!({"shape":"bell","freq":1000.0,"gainDb":gain,"q":1.0});
    json!({"type":"change","change":{"id":id,"family":"parameter","title":"EQ Eight · 3 Gain A 0.00 dB → -4.00 dB","state":"applied","at":1,
        "picture":{"kind":"eq","before":[band(0.0)],"after":[band(gain)],
            "values":[{"name":"3 Gain A","from":0.0,"to":gain,"fromText":"0.00 dB","toText":"-4.00 dB","asked":"-4.5 dB"}]}}})
}

fn tempo_change(id: &str) -> Value {
    json!({"type":"change","change":{"id":id,"family":"tempo","title":"Tempo 120 → 124 BPM","state":"applied","at":2,"from":120.0,"to":124.0,
        "picture":{"kind":"values","values":[{"name":"Tempo","from":120.0,"to":124.0,"min":20.0,"max":999.0,"fromText":"120 BPM","toText":"124 BPM"}]}}})
}

/// A note whose velocity alone changed, as Live sends it (#643): its start and length as Live keeps a triplet's
/// (2.6666666666666665, 0.3333333333333333), and as the model's patch restated them (2.6667, 0.3333).
fn velocity_change(id: &str) -> Value {
    json!({"type":"change","change":{"id":id,"family":"clip","title":"Bassline · velocity of 1 note","state":"applied","at":3,
        "picture":{"kind":"notes","length":4.0,
            "before":[[2.6666666666666665, 36.0, 0.3333333333333333, 90.0]],
            "after":[[2.6667, 36.0, 0.3333, 110.0]]}}})
}

/// The first row under HISTORY's strip with `text`.
fn history_row(lines: &[String], text: &str) -> usize {
    let strip = lines.iter().position(|line| line.contains("HISTORY")).expect("HISTORY");
    strip
        + lines[strip..].iter().position(|line| line.contains(text)).unwrap_or_else(|| panic!("{text:?} in HISTORY:\n{}", lines.join("\n")))
}

fn braille(lines: &[String]) -> bool {
    lines.iter().any(|line| line.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)))
}

case!(a_history_row_opens_to_its_picture_and_closes_again, async {
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    h.emit(eq_change("c1", -4.));
    h.emit(tempo_change("c2"));
    h.has("Tempo 120 → 124 BPM");
    assert!(!has(&h.screen(), "asked -4.5 dB"), "closed until opened");
    // Into the tabs: the newest change first, then the EQ move.
    let opened = |h: &Harness, text: &str| {
        let lines = h.screen();
        let strip = lines.iter().position(|line| line.contains("HISTORY")).unwrap();
        lines[strip..].iter().any(|line| line.contains(text))
    };
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    assert!(opened(&h, "Tempo 120 BPM → 124 BPM"), "{}", h.screen().join("\n"));
    h.type_text("\x1b[D").await;
    assert!(!opened(&h, "Tempo 120 BPM → 124 BPM"));
    h.type_text("\x1b[B").await;
    h.type_text(" ").await;
    h.wait_for("3 Gain A 0.00 dB → -4.00 dB").await;
    h.has("asked -4.5 dB");
    // The keyboard stays on the change that opened, not on a row of its picture.
    assert!(h.selected_line().is_some_and(|line| line.contains("✓ EQ Eight")), "{:?}", h.selected_line());
    assert!(braille(&h.screen()), "the curve, drawn in braille here");
    // Enter closes it again; nothing was undone.
    h.type_text("\r").await;
    h.wait_until_hidden("asked -4.5 dB").await;
    assert!(!h.calls().iter().any(|call| call.starts_with("undo:")));
    h.close().await;
});

case!(a_note_whose_velocity_alone_changed_is_kept_not_moved, async {
    // As a saved record keeps it, to 3 decimals: the note stays the note it was, as it does once Kumi starts again.
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    h.emit(velocity_change("c1"));
    h.has("Bassline · velocity of 1 note");
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.wait_for("1 note → 1 note").await;
    let screen = h.screen().join("\n");
    assert!(!screen.contains("· 1 new") && !screen.contains("· 1 gone"), "the note was neither added nor taken away:\n{screen}");
    h.close().await;
});

case!(a_click_on_a_rows_words_opens_it_and_undo_stays_on_its_label, async {
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    h.emit(tempo_change("c2"));
    let lines = h.screen();
    let row = history_row(&lines, "Tempo 120 → 124 BPM");
    h.type_text(&click(&lines, row, "Tempo 120")).await;
    let lines = h.screen();
    history_row(&lines, "Tempo 120 BPM → 124 BPM");
    assert!(!h.calls().iter().any(|call| call.starts_with("undo:")));
    h.type_text(&click(&lines, row, "undo")).await;
    h.wait_for_call("undo:c2").await;
    h.close().await;
});

case!(a_curve_that_comes_after_its_change_is_drawn_again, async {
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    // The change is told with its values; its curve comes after (read once it was applied).
    let mut first = eq_change("c1", -4.);
    first["change"]["picture"] =
        json!({"kind":"values","values":[{"name":"3 Gain A","from":0.0,"to":-4.0,"fromText":"0.00 dB","toText":"-4.00 dB"}]});
    h.emit(first);
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.wait_for("3 Gain A 0.00 dB → -4.00 dB").await;
    assert!(!braille(&h.screen()));
    h.emit(eq_change("c1", -4.));
    h.wait_for("asked -4.5 dB").await;
    assert!(braille(&h.screen()), "the curve, now it's here");
    h.close().await;
});

case!(the_narrow_dock_shows_the_last_changes_picture_in_a_row, async {
    // (the two-line strip, below 24 rows)
    let h = Harness::new(80, 23);
    h.start().await;
    h.connect();
    h.emit(eq_change("c1", -4.));
    let lines = h.screen();
    let row = lines.iter().position(|line| line.contains("✓ EQ Eight")).expect("the dock's last change");
    assert!(braille(&lines[row..=row]), "{}", lines[row]);
    assert!(lines[row].contains("undo"));
    h.close().await;
});

case!(the_drawers_now_shows_the_last_changes_picture_under_it, async {
    let h = Harness::new(80, 30);
    h.start().await;
    h.connect();
    h.emit(eq_change("c1", -4.));
    let lines = h.screen();
    let row = lines.iter().position(|line| line.contains("✓ EQ Eight")).expect("NOW's last change");
    assert!(lines[row].trim_end().ends_with("undo") && !braille(&lines[row..=row]), "{}", lines[row]);
    // The curve under it, as tall as the drawer holds (its 7 rows: the tabs, NOW, the change, then 4), then what moved.
    assert!(braille(&lines[row + 1..row + 4]), "{}", lines.join("\n"));
    assert!(lines[row + 4].contains("3 Gain A 0.00 dB → -4.00 dB"), "{}", lines.join("\n"));
    h.close().await;
});

case!(images_go_up_only_where_the_terminal_shows_them, async {
    // Cells: no image, ever.
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    h.emit(eq_change("c1", -4.));
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.wait_for("asked -4.5 dB").await;
    assert!(!h.output.text.borrow().contains("\x1b_G"));
    h.close().await;
    // Kitty's placeholders: the image is uploaded, quietly, before the frame that shows it, and taken down at the end.
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| o.graphics = Some(Graphics::Kitty));
    h.start().await;
    h.connect();
    h.emit(eq_change("c1", -4.));
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.wait_for("asked -4.5 dB").await;
    let written = h.output.text.borrow().clone();
    let upload = written.find("\x1b_Ga=T,U=1,i=").expect("an upload");
    assert!(written[upload..].starts_with("\x1b_Ga=T,U=1,i="), "{}", &written[upload..upload + 60]);
    assert!(written[upload..].split("\x1b\\").next().unwrap().contains(",q=2,"));
    let placeholder = written.find('\u{10EEEE}').expect("placeholder cells");
    assert!(upload < placeholder, "the image goes up first");
    // HISTORY's picture (6 rows) and NOW's (2 rows, as the change landed): each uploaded once, however many frames show it.
    assert_eq!(written.matches(",r=6,o=z").count(), 1, "HISTORY's, once");
    assert_eq!(written.matches(",r=2,o=z").count(), 1, "NOW's, once");
    h.close().await;
    let written = h.output.text.borrow();
    assert_eq!(written.matches("\x1b_Ga=d,d=I,").count(), 2, "both taken down as Kumi closes");
});

case!(on_a_basic_console_pictures_are_words, async {
    let h = Harness::with(160, 40, Rc::new(Control::default()), |o| o.graphics = Some(Graphics::Words));
    h.start().await;
    h.connect();
    // A new clip: no braille roll in NOW, its title says it.
    h.emit(json!({"type":"change","change":{"id":"c3","family":"clip","title":"New MIDI clip “Chord” · 3 notes","state":"applied","at":3,
        "clip":{"length":4.0,"notes":[{"pitch":60.0,"start":0.0,"duration":1.0,"velocity":100.0}]}}}));
    h.emit(eq_change("c1", -4.));
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.wait_for("3 Gain A 0.00 dB → -4.00 dB").await;
    assert!(!braille(&h.screen()), "{}", h.screen().join("\n"));
    h.close().await;
});

case!(a_change_from_before_pictures_shows_what_it_has, async {
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    // Saved before Kumi drew pictures: a value's before and after in its range.
    h.emit(json!({"type":"change","change":{"id":"c9","family":"mixer","title":"Bass volume down","state":"applied","at":9,"from":0.85,"to":0.6,"range":[0.0,1.0]}}));
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.wait_for("░").await;
    assert!(!has(&h.screen(), "No picture for this change"));
    h.close().await;
});

case!(a_typed_u_undoes_only_a_change_whose_picture_is_open, async {
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    h.emit(tempo_change("c2"));
    // In the tabs, on a closed change: a typed u is a word for the box.
    h.type_text("\x1b[Z").await;
    h.type_text("u").await;
    h.type_text("se more reverb").await;
    h.has("use more reverb");
    assert!(!h.calls().iter().any(|call| call.starts_with("undo:")), "{:?}", h.calls());
    h.type_text("\x1b[H").await;
    // Opened on purpose, u undoes it.
    h.type_text("\x15").await;
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.wait_for("Tempo 120 BPM → 124 BPM").await;
    h.type_text("u").await;
    h.wait_for_call("undo:c2").await;
    h.close().await;
});

case!(arrows_open_and_close_and_never_leave_the_row, async {
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    h.emit(tempo_change("c2"));
    let shown = |h: &Harness| {
        let lines = h.screen();
        let strip = lines.iter().position(|line| line.contains("HISTORY")).unwrap();
        lines[strip..].iter().any(|line| line.contains("Tempo 120 BPM → 124 BPM"))
    };
    h.type_text("\x1b[Z").await;
    h.type_text("\x1b[C").await;
    assert!(shown(&h), "→ opens");
    h.type_text("\x1b[C").await;
    assert!(shown(&h), "→ on an open row leaves it open");
    h.type_text("\x1b[D").await;
    assert!(!shown(&h), "← closes");
    h.type_text("\x1b[D").await;
    assert!(h.selected_line().is_some_and(|line| line.contains("Tempo 120 → 124 BPM")), "← on a closed row stays: {:?}", h.selected_line());
    h.close().await;
});

case!(opening_a_change_under_kept_things_keeps_it_in_view_and_selected, async {
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    for (id, text) in [("s1", "The Reese is the main bass"), ("s2", "Drums sit at -6 LUFS")] {
        h.emit(json!({"type":"remembered","scope":"set","note":{"id":id,"text":text,"at":1}}));
    }
    h.emit(eq_change("c1", -4.));
    h.emit(tempo_change("c2"));
    h.type_text("\x1b[Z").await;
    for _ in 0..6 {
        if h.selected_line().is_some_and(|line| line.contains("EQ Eight")) {
            break;
        }
        h.type_text("\x1b[B").await;
    }
    assert!(h.selected_line().is_some_and(|line| line.contains("EQ Eight")), "{:?}", h.selected_line());
    h.type_text("\r").await;
    h.wait_for("3 Gain A 0.00 dB → -4.00 dB").await;
    assert!(h.selected_line().is_some_and(|line| line.contains("✓ EQ Eight")), "{:?}", h.selected_line());
    h.close().await;
});

case!(a_value_change_opens_to_its_bar_and_words, async {
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    h.emit(tempo_change("c2"));
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    let lines = h.screen();
    let strip = lines.iter().position(|line| line.contains("HISTORY")).unwrap();
    assert!(
        lines[strip..].iter().any(|line| line.contains("█") && line.contains("Tempo 120 BPM → 124 BPM")),
        "{}",
        lines[strip..].join("\n")
    );
    h.close().await;
});

case!(a_fixed_phrases_change_opens_to_its_picture_like_any_other, async {
    let h = Harness::new(160, 40);
    h.control.set("instant", json!(true));
    h.control.set("instant-change", tempo_change("c1")["change"].clone());
    h.start().await;
    h.connect();
    h.type_text("tempo 124\r").await;
    h.wait_for_call("instant:tempo 124").await;
    h.has("Tempo 120 → 124 BPM · instant");
    // In HISTORY the phrase's change opens as any other does: its value on a bar, with its words.
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    let lines = h.screen();
    let row = history_row(&lines, "Tempo 120 → 124 BPM");
    assert!(lines[row + 1..].iter().any(|line| line.contains("Tempo 120 BPM → 124 BPM")), "{}", lines.join("\n"));
    h.close().await;
});

case!(a_plans_change_across_tracks_is_one_entry_with_one_picture_and_one_undo, async {
    let h = Harness::new(160, 40);
    h.start().await;
    h.connect();
    let muted = |id: &str, track: &str, at: i64| {
        json!({"type":"change","change":{"id":id,"family":"mixer","title":format!("{track} muted"),"state":"applied","at":at,
            "track":{"name":track},"batch":"p1:set_mixer",
            "picture":{"kind":"mix","before":{"mute":false},"after":{"mute":true}}}})
    };
    h.emit(muted("c1", "Drums", 1));
    h.emit(muted("c2", "Bass", 2));
    h.emit(muted("c3", "Keys", 3));
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.wait_for("Drums, Bass and Keys muted").await;
    let lines = h.screen();
    let strip = lines.iter().position(|line| line.contains("HISTORY")).expect("HISTORY");
    assert!(!lines[strip..].iter().any(|line| line.contains("Bass muted") && !line.contains("Keys")), "one entry:\n{}", lines.join("\n"));
    let row = history_row(&lines, "Drums, Bass and Keys muted");
    for track in ["Drums", "Bass", "Keys"] {
        assert!(lines[row + 1..].iter().any(|line| line.contains(&format!("{track} · Mute off → on"))), "{track}:\n{}", lines.join("\n"));
    }
    // Its undo takes back each change, newest first.
    h.type_text("u").await;
    h.wait_for_call("undo:c1").await;
    let undone: Vec<String> = h.calls().into_iter().filter(|call| call.starts_with("undo:")).collect();
    assert_eq!(undone, ["undo:c3", "undo:c2", "undo:c1"]);
    h.close().await;
});
