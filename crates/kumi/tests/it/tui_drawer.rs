//! Below 100 columns, with 24 rows or more, the pane is a drawer under the conversation: folded to its row of tabs, it
//! opens to NOW when a change lands (its picture, its undo), to the tabs when Kumi turns them, and wherever the producer
//! clicks or the keyboard goes; what Kumi opened folds after a quiet spell. It never takes the keyboard. ctrl+\ opens
//! and closes it, and puts the pane away at other sizes: the side pane at full width, the two-line strip below 24 rows.
#[path = "../support/tui_app.rs"]
mod support;
use serde_json::{json, Value};
use std::{rc::Rc, time::Duration};
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

/// The drawer's row of tabs.
fn tabs(lines: &[String]) -> usize {
    lines.iter().position(|line| line.starts_with("  NOW   FOCUS")).unwrap_or_else(|| panic!("no drawer:\n{}", lines.join("\n")))
}

/// A notes change, with its picture.
fn change(id: &str, title: &str) -> Value {
    let note = |start: f64, pitch: f64, duration: f64| json!([start, pitch, duration, 100.0]);
    json!({"type":"change","change":{"id":id,"family":"clip","title":title,"state":"applied","at":1,
        "track":{"name":"Keys","color":"#5ec1f7"},
        "picture":{"kind":"notes","length":8.0,"before":[note(0.,60.,2.),note(2.,62.,2.)],"after":[note(0.,60.,1.),note(1.,67.,1.),note(2.,65.,2.)]}}})
}

fn focus() -> Value {
    json!({"type":"focus","focus":{"track":{"name":"Bass","color":"#f59a3c","kind":"midi"},"device":"Operator","detail":"Device","view":"Session","parameter":{"name":"Filter Freq","value":"1.20 kHz","owner":"Operator"}}})
}

async fn click(h: &Harness, row: usize, text: &str) {
    let lines = h.screen();
    h.type_text(&support::click(&lines, row, text)).await;
}

case!(below_100_columns_the_pane_is_a_drawer_folded_to_its_row_of_tabs, async {
    for (width, height) in [(80, 24), (80, 30), (99, 40)] {
        let h = Harness::new(width, height);
        h.start().await;
        h.connect();
        let lines = h.screen();
        let row = tabs(&lines);
        assert!(lines[row].contains("   HISTORY ─") && lines[row].trim_end().ends_with("ctrl+\\ opens"), "{}", lines[row]);
        // Folded: one row, a blank one, then the box; the side pane's headings are nowhere.
        assert!(lines[row + 3].contains("Ask Kumi about your Set"), "{}", lines.join("\n"));
        assert!(!has(&lines, "FOCUS ·") && !has(&lines, "Your selection will show here"));
        h.close().await;
    }
});

case!(below_24_rows_the_two_line_strip_stays_and_ctrl_backslash_puts_it_away, async {
    let h = Harness::new(80, 23);
    h.start().await;
    h.connect();
    h.emit(focus());
    let lines = h.screen();
    assert!(!has(&lines, "NOW   FOCUS") && !has(&lines, "ctrl+\\"), "{}", lines.join("\n"));
    h.has("■ Bass › Operator › Filter Freq · 1.20 kHz");
    h.type_text("\x1c").await;
    assert!(!has(&h.screen(), "■ Bass"), "put away");
    h.type_text("\x1c").await;
    h.has("■ Bass › Operator › Filter Freq · 1.20 kHz");
    h.close().await;
});

case!(a_change_opens_the_drawer_to_now_with_its_picture_and_its_undo, async {
    for (width, height) in [(80, 24), (80, 30), (90, 40)] {
        let h = Harness::new(width, height);
        h.start().await;
        h.connect();
        h.emit(change("c1", "Chords · 3 notes moved"));
        let lines = h.screen();
        let row = tabs(&lines);
        assert!(lines[row].trim_end().ends_with("ctrl+\\ closes"), "{}", lines[row]);
        assert!(lines[row + 1].trim() == "Ready", "{}", lines.join("\n"));
        assert!(lines[row + 2].contains("■ Chords · 3 notes moved") && lines[row + 2].trim_end().ends_with("undo"), "{}", lines[row + 2]);
        assert!(!lines[row + 3].trim().is_empty(), "its picture:\n{}", lines.join("\n"));
        // 6 to 10 rows by the window's height, then a blank row over the box.
        let tall = (height / 4).clamp(6, 10) as usize;
        assert!(lines[row + tall].trim().is_empty() && lines[row + tall + 2].contains("Ask Kumi about your Set"), "{}", lines.join("\n"));
        click(&h, row + 2, "undo").await;
        assert!(h.calls().contains(&"undo:c1".into()), "{:?}", h.calls());
        h.close().await;
    }
});

case!(the_drawer_never_takes_the_keyboard, async {
    let h = Harness::new(80, 30);
    h.start().await;
    h.connect();
    h.emit(change("c1", "Chords · 3 notes moved"));
    h.has("ctrl+\\ closes");
    h.type_text("tighten the hats\r").await;
    assert!(h.calls().contains(&"submit:tighten the hats".into()), "{:?}", h.calls());
    assert!(!h.has_selection_highlight(), "nothing in the drawer has the keyboard");
    h.close().await;
});

case!(ctrl_backslash_closes_the_drawer_and_kumi_leaves_it_closed_until_it_opens_again, async {
    let h = Harness::new(80, 30);
    h.start().await;
    h.connect();
    h.emit(change("c1", "Chords · 3 notes moved"));
    h.has("ctrl+\\ closes");
    h.type_text("\x1c").await;
    let lines = h.screen();
    assert!(lines[tabs(&lines)].trim_end().ends_with("ctrl+\\ opens") && !has(&lines, "Chords · 3 notes moved"), "{}", lines.join("\n"));
    // Closed by the producer: a change only counts in HISTORY.
    h.emit(change("c2", "Bass · 2 notes added"));
    let lines = h.screen();
    assert!(!has(&lines, "Bass · 2 notes added") && lines[tabs(&lines)].contains("HISTORY 2"), "{}", lines.join("\n"));
    // Opened again, it shows where it was: NOW, with the latest change.
    h.type_text("\x1c").await;
    let lines = h.screen();
    let row = tabs(&lines);
    assert!(lines[row].trim_end().ends_with("ctrl+\\ closes") && lines[row + 2].contains("Bass · 2 notes added"), "{}", lines.join("\n"));
    h.close().await;
});

case!(what_kumi_opened_folds_after_a_quiet_spell_and_what_the_producer_opened_stays, async {
    tokio::time::pause();
    let h = Harness::new(80, 30);
    h.start().await;
    h.connect();
    h.emit(change("c1", "Chords · 3 notes moved"));
    h.has("ctrl+\\ closes");
    tokio::time::advance(Duration::from_millis(6_000)).await;
    delay(5).await;
    // Another change keeps it open for another spell.
    h.emit(change("c2", "Bass · 2 notes added"));
    h.has("Bass · 2 notes added");
    tokio::time::advance(Duration::from_millis(6_000)).await;
    delay(5).await;
    h.has("ctrl+\\ closes");
    tokio::time::advance(Duration::from_millis(4_100)).await;
    delay(5).await;
    let lines = h.screen();
    assert!(lines[tabs(&lines)].trim_end().ends_with("ctrl+\\ opens"), "folded:\n{}", lines.join("\n"));
    // Opened by the producer, it stays.
    h.type_text("\x1c").await;
    tokio::time::advance(Duration::from_millis(30_000)).await;
    delay(5).await;
    h.has("ctrl+\\ closes");
    h.close().await;
});

case!(a_click_on_a_tab_opens_the_drawer_there, async {
    let h = Harness::new(80, 30);
    h.start().await;
    h.connect();
    h.emit(focus());
    click(&h, tabs(&h.screen()), "FOCUS").await;
    let lines = h.screen();
    let row = tabs(&lines);
    assert!(lines[row + 1].contains("■ Bass › Operator › Filter Freq · 1.20 kHz"), "{}", lines.join("\n"));
    click(&h, row, "HISTORY").await;
    let lines = h.screen();
    assert!(lines[tabs(&lines) + 1].contains("Nothing changed yet"), "{}", lines.join("\n"));
    click(&h, tabs(&lines), "NOW").await;
    let lines = h.screen();
    assert!(lines[tabs(&lines) + 1].trim() == "Ready", "{}", lines.join("\n"));
    // A disconnect clears FOCUS, as it does in the pane.
    click(&h, tabs(&lines), "FOCUS").await;
    h.emit(json!({"type":"connection","state":"disconnected"}));
    let lines = h.screen();
    assert!(!has(&lines, "■ Bass") && lines[tabs(&lines) + 1].contains("Live isn't connected"), "{}", lines.join("\n"));
    h.close().await;
});

case!(the_keys_into_the_pane_open_the_drawer_there_and_nothing_turns_it_under_them, async {
    let h = Harness::new(80, 30);
    h.start().await;
    h.connect();
    h.emit(change("c1", "Chords · 3 notes moved"));
    // Shift-Tab: into HISTORY, the drawer open on it.
    h.type_text("\x1b[Z").await;
    assert!(h.selected_line().is_some_and(|line| line.contains("Chords · 3 notes moved")), "{}", h.screen().join("\n"));
    // A change while the keyboard is there leaves the drawer on HISTORY, the keyboard on its row.
    h.emit(change("c2", "Bass · 2 notes added"));
    let lines = h.screen();
    let row = tabs(&lines);
    assert!(lines[row + 1].contains("Bass · 2 notes added") && lines[row + 2].contains("Chords · 3 notes moved"), "{}", lines.join("\n"));
    assert!(h.selected_line().is_some_and(|line| line.contains("Chords · 3 notes moved")));
    // Esc gives the keyboard back to the box; ctrl+\ then closes the drawer.
    h.type_text("\x1b").await;
    h.type_text("\x1c").await;
    h.type_text("hello").await;
    let lines = h.screen();
    assert!(lines[tabs(&lines)].trim_end().ends_with("ctrl+\\ opens") && has(&lines, "hello"), "{}", lines.join("\n"));
    h.close().await;
});

case!(tab_into_focuss_tree_opens_the_drawer_on_focus, async {
    let c = Rc::new(Control::default());
    *c.tree.borrow_mut() = Some(
        serde_json::from_value(json!({"trackRef":"3:track:1","devices":[
            {"ref":"d1","name":"Operator","className":"Operator","deviceType":"instrument"},
            {"ref":"d2","name":"Saturator","className":"Saturator","deviceType":"audio_effect"}]}))
        .unwrap(),
    );
    let h = Harness::with(80, 30, c, |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Bass","color":"#f59a3c","kind":"midi"},"trackRef":"3:track:1","device":"Operator","detail":"Device","view":"Session"}}));
    h.wait_for_call("tree:3:track:1").await;
    h.type_text("\t").await;
    let lines = h.screen();
    let row = tabs(&lines);
    assert!(lines[row].starts_with("  NOW   FOCUS · Device"), "{}", lines[row]);
    assert!(h.selected_line().is_some_and(|line| line.contains("Operator")), "{}", lines.join("\n"));
    // Enter points at the row, as in the pane.
    h.type_text("\x1b[B\r").await;
    h.wait_for("Saturator  ×").await;
    h.close().await;
});

case!(a_focus_body_taller_than_the_drawer_stops_at_its_edge, async {
    let c = Rc::new(Control::default());
    let note = |pitch: i32, start: f64| json!({"pitch":pitch,"start":start,"duration":1.0,"velocity":96});
    *c.clip.borrow_mut() = Some(
        serde_json::from_value(json!({"slotRef":"3:clip_slot:2:0","name":"Chords","length":8,
            "notes":[note(48,0.),note(55,1.),note(60,2.),note(64,3.),note(67,4.)],
            "lanes":[{"name":"Mod wheel","max":127.0,"points":[[0.0,10.0],[4.0,90.0]]},{"name":"Pitch","max":127.0,"points":[[0.0,64.0]]}]}))
        .unwrap(),
    );
    let h = Harness::with(80, 30, c, |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Keys","color":"#5ec1f7","kind":"midi"},"slotRef":"3:clip_slot:2:0","clip":"Chords","detail":"Clip","view":"Session"}}));
    h.wait_for_call("clip:3:clip_slot:2:0").await;
    click(&h, tabs(&h.screen()), "FOCUS").await;
    let lines = h.screen();
    let row = tabs(&lines);
    assert!(lines[row].starts_with("  NOW   FOCUS · Clip") && lines[row + 1].contains("Chords"), "{}", lines.join("\n"));
    // The drawer's 7 rows hold the header and the notes; what's past them is painted over: a blank row, then the box.
    assert!(lines[row + 7].trim().is_empty() && lines[row + 9].contains("Ask Kumi about your Set"), "{}", lines.join("\n"));
    assert!(!has(&lines[row + 7..], "Mod wheel"), "{}", lines.join("\n"));
    h.close().await;
});

case!(kumi_turning_the_tabs_to_goal_opens_the_drawer_on_them, async {
    let c = Rc::new(Control::default());
    c.set("goal", json!(true));
    let h = Harness::with(80, 30, c, |_| {});
    h.start().await;
    h.connect();
    h.type_text("/goal master this to -9 LUFS\r").await;
    h.wait_for_call("goal:master this to -9 LUFS").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"objective","objective":"master this to -9 LUFS","state":"running","turns":1,"turnBudget":12,"elapsedMs":65_020,"budgetMs":3_600_000,"verdict":"continue","reason":"","next":"","measured":false}));
    let lines = h.screen();
    let row = tabs(&lines);
    assert!(lines[row + 1].contains("master this to -9 LUFS"), "{}", lines.join("\n"));
    // While Kumi works, the row's end says so instead of the key.
    assert!(lines[row].contains("● goal · turn 2/12"), "{}", lines[row]);
    h.close().await;
});

case!(at_full_width_ctrl_backslash_puts_the_side_pane_away_and_brings_it_back, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.has("Your selection will show here");
    h.type_text("\x1c").await;
    let lines = h.screen();
    assert!(!has(&lines, "Your selection will show here") && !has(&lines, "HISTORY"), "{}", lines.join("\n"));
    // The conversation and the box take the whole width.
    let hint = lines.iter().find(|line| line.contains("enter to send")).unwrap();
    assert_eq!(hint.trim_end().chars().count(), 117, "{hint}");
    // Shift-Tab, into the tabs, brings it back too.
    h.type_text("\x1b[Z").await;
    h.has("Your selection will show here");
    h.type_text("\x1b").await;
    h.type_text("\x1c").await;
    h.type_text("\x1c").await;
    h.has("Your selection will show here");
    h.close().await;
});

case!(the_focus_tab_keeps_its_body_fresh_only_while_it_shows, async {
    tokio::time::pause();
    let c = Rc::new(Control::default());
    *c.arrangement.borrow_mut() = Some(
        serde_json::from_value(json!({"length":128,"position":68,"playing":true,"locators":[{"name":"Drop","position":96}]})).unwrap(),
    );
    let reads = |h: &Harness| h.calls().iter().filter(|call| *call == "arrangement").count();
    let h = Harness::with(80, 30, c, |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Keys","color":"#5ec1f7","kind":"midi"},"trackRef":"3:track:1","view":"Arrangement"}}));
    delay(5).await;
    // Folded: read as the focus changed, not again.
    let read = reads(&h);
    tokio::time::advance(Duration::from_millis(4_600)).await;
    delay(5).await;
    assert_eq!(reads(&h), read, "{:?}", h.calls());
    click(&h, tabs(&h.screen()), "FOCUS").await;
    h.has("FOCUS · Arrangement");
    tokio::time::advance(Duration::from_millis(1_600)).await;
    delay(5).await;
    assert!(reads(&h) > read, "the FOCUS tab is kept fresh: {:?}", h.calls());
    h.close().await;
});
