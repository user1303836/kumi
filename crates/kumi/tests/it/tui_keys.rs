//! Keys as actions: where actions share a key, each state gets what the old if-chain gave it, with the modifiers it
//! ignored; and keys.json's keys come first.
#[path = "../support/tui_app.rs"]
mod support;
use kumi::terminal::Terminal;
use serde_json::json;
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

/// What looking back through what was sent finds: a history of the messages sent in this test only.
fn history() -> (tempfile::TempDir, Rc<std::cell::RefCell<kumi::history::InputHistory>>) {
    let folder = tempfile::tempdir().unwrap();
    let history = kumi::history::open_input_history(Some(folder.path().join("input-history")), vec![]);
    (folder, Rc::new(std::cell::RefCell::new(history)))
}

case!(tab_sends_after_the_answer_only_while_kumi_works_with_words, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    // Idle, with words: nothing is held, and the words stay.
    h.type_text("make it darker\t").await;
    assert!(!has(&h.screen(), "after this answer") && has(&h.screen(), "make it darker"));
    h.emit(json!({"type":"state","state":"running"}));
    // Ctrl+tab isn't tab: nothing happens.
    h.type_text("\x1b[9;5u").await;
    assert!(!has(&h.screen(), "after this answer") && has(&h.screen(), "make it darker"));
    // Working, with words: tab holds them for after the answer.
    h.type_text("\t").await;
    assert!(h.screen().iter().any(|s| s.contains("↳ make it darker") && s.contains("after this answer")));
    // Shift+tab goes into the tabs, holding nothing.
    h.type_text("louder\x1b[Z").await;
    assert!(!h.screen().iter().any(|s| s.contains("↳ louder")));
    h.close().await;
});

case!(alt_up_takes_back_a_waiting_message_and_with_none_waiting_is_up, async {
    let (_folder, history) = history();
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.history = Some(history));
    h.start().await;
    h.connect();
    h.type_text("first thing\r").await;
    h.wait_for_call("submit:first thing").await;
    // Nothing waits: alt+↑ (ctrl+alt+↑ too) goes back through what was sent, as ↑ does, and enter sends it again.
    h.type_text("\x1b[1;7A\r").await;
    h.wait_for_calls("submit:first thing", 2).await;
    // A message waiting after the answer comes back into the box.
    h.emit(json!({"type":"state","state":"running"}));
    h.type_text("second\t").await;
    assert!(h.screen().iter().any(|s| s.contains("↳ second")));
    h.type_text("\x1b[1;3A").await;
    assert!(!h.screen().iter().any(|s| s.contains("↳ second")) && has(&h.screen(), "second"));
    h.close().await;
});

case!(ctrl_d_quits_from_an_empty_box_while_kumi_is_idle_and_otherwise_deletes, async {
    let h = Harness::new(120, 36);
    let done = h.app.run();
    delay(5).await;
    h.connect();
    // With words, it deletes the character after the cursor; ctrl+shift+d too.
    h.type_text("abcd\x01\x04").await;
    assert!(has(&h.screen(), "bcd") && !has(&h.screen(), "abcd"));
    h.type_text("\x1b[100;6u").await;
    assert!(has(&h.screen(), "cd") && !has(&h.screen(), "bcd"));
    // An empty box while Kumi is idle: it quits.
    h.type_text("\x05\x15\x04").await;
    assert_eq!(done.await, 0);
    h.close().await;
});

case!(esc_closes_the_menu_before_anything_else, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("/he").await;
    h.wait_for("enter to choose · esc to close").await;
    // Shift+esc is esc.
    h.type_text("\x1b[27;2u").await;
    h.wait_until_hidden("enter to choose · esc to close").await;
    assert!(has(&h.screen(), "/he"), "the words stay");
    h.close().await;
});

case!(a_key_that_doesnt_apply_lets_the_next_have_it, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    // Without background jobs, ctrl+alt+b is alt+b: a word left.
    h.type_text("make it darker\x1b\x02x").await;
    assert!(has(&h.screen(), "make it xdarker"), "{:?}", h.screen());
    h.type_text("\x05\x15").await;
    // Shift+left is left; ctrl+left a word left.
    h.type_text("ab\x1b[1;2Dx").await;
    assert!(has(&h.screen(), "axb"));
    h.type_text("\x05\x15quieter drums\x1b[1;5Dthe ").await;
    assert!(has(&h.screen(), "quieter the drums"));
    h.close().await;
});

case!(enter_sends_and_with_alt_or_shift_starts_a_new_line, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    // Alt+enter and shift+enter (where the terminal tells it apart) start a new line, and ctrl+j too; ctrl+enter sends.
    h.type_text("one\x1b\rtwo\x1b[13;2uthree\nfour\x1b[13;5u").await;
    h.wait_for_call("submit:one\ntwo\nthree\nfour").await;
    h.close().await;
});

case!(a_key_the_search_doesnt_take_stops_it_with_what_it_found_and_goes_on, async {
    let (_folder, history) = history();
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.history = Some(history));
    h.start().await;
    h.connect();
    h.type_text("older words\r").await;
    h.wait_for_call("submit:older words").await;
    // Looking back finds it; ctrl+a isn't the search's, so it puts the message in the box and goes to the line's start.
    h.type_text("\x12\x01x").await;
    assert!(has(&h.screen(), "xolder words"), "{:?}", h.screen());
    h.close().await;
});

case!(keys_json_keys_come_first_in_the_box, async {
    let file = json!({ "composer.undo": ["ctrl+z"], "app.redraw": ["ctrl+d"] });
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.keys = Some(Ok(file)));
    let done = h.app.run();
    delay(5).await;
    h.connect();
    // (the notice wraps at this width)
    h.has("keys.json gives ctrl+d to app.redraw in the box, ahead of app.quit and");
    // Ctrl+z undoes, and ctrl+_ (undo's own key) no longer does.
    h.type_text("abc").await;
    h.type_text("\x1f").await;
    assert!(has(&h.screen(), "abc"));
    h.type_text("\x1a").await;
    assert!(!has(&h.screen(), "abc"));
    // Ctrl+d is app.redraw's, which always applies: an empty idle box doesn't quit.
    h.type_text("\x04").await;
    delay(5).await;
    h.has("Ask Kumi about your Set");
    h.type_text("\x03").await;
    assert_eq!(done.await, 0);
    h.close().await;
});

case!(a_search_that_ends_on_a_command_gives_the_menu_its_keys, async {
    let (_folder, history) = history();
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.history = Some(history));
    h.start().await;
    h.connect();
    h.type_text("older words\r").await;
    h.wait_for_call("submit:older words").await;
    h.type_text("/he").await;
    h.wait_for("enter to choose · esc to close").await;
    // Looking back for words nothing sent has, then ↑: the search ends with the draft, a command, and ↑ moves its menu
    // rather than bringing back what was sent.
    h.type_text("\x12zzz").await;
    h.wait_until_hidden("enter to choose · esc to close").await;
    h.type_text("\x1b[A").await;
    h.wait_for("enter to choose · esc to close").await;
    h.close().await;
});

case!(a_panels_key_that_doesnt_apply_lets_the_next_have_it, async {
    let c = Rc::new(Control::default());
    c.set("aside", json!("Up to a minute."));
    let file = json!({ "aside.earlier": ["q"], "aside.close": ["q"] });
    let h = Harness::with(120, 36, c, |o| o.keys = Some(Ok(file)));
    h.start().await;
    h.connect();
    h.type_text("/btw how long is the tail?\r").await;
    h.wait_for("btw · how long is the tail?").await;
    // The only side answer has none before it, so q goes on to closing it.
    h.type_text("q").await;
    h.wait_until_hidden("btw ·").await;
    h.close().await;
});

case!(keys_one_after_another_take_their_second_key_before_what_it_does_alone, async {
    let file = json!({ "app.help": ["ctrl+x ctrl+e"] });
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.keys = Some(Ok(file)));
    h.start().await;
    h.connect();
    // Alone, ctrl+e is still the line's end.
    h.type_text("ab\x01\x05c").await;
    assert!(has(&h.screen(), "abc"), "{:?}", h.screen());
    h.type_text("\x05\x15").await;
    // After ctrl+x, it finishes the pair, and the help opens.
    h.type_text("\x18\x05").await;
    h.wait_for("Keys and commands").await;
    h.close().await;
});

#[test]
fn every_action_is_in_the_keys_docs_in_each_language() {
    for (language, doc) in [
        ("en", include_str!("../../../../docs/en/KUMI_TUI.md")),
        ("ja", include_str!("../../../../docs/ja/KUMI_TUI.md")),
        ("zh-CN", include_str!("../../../../docs/zh-CN/KUMI_TUI.md")),
    ] {
        for action in kumi::tui::keymap::ACTIONS {
            assert!(doc.contains(&format!("`{}`", action.id)), "{} isn't in {language}'s KUMI_TUI.md", action.id);
        }
    }
}
