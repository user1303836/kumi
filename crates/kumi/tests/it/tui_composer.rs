//! The composer: undo, the kill ring, history kept from ctrl+c, and typing that stays fast after a long paste.
#[path = "../support/tui_app.rs"]
mod support;
use kumi::tui::{editor::Editor, width::graphemes};
use std::{
    rc::Rc,
    time::{Duration, Instant},
};
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

fn typed(editor: &mut Editor, text: &str) {
    for character in text.chars() {
        editor.type_text(&character.to_string());
    }
}

#[test]
fn a_run_of_typing_a_paste_and_a_run_of_deleting_are_one_step_each() {
    let mut e = Editor::new();
    typed(&mut e, "make the kick");
    e.insert(" louder");
    for _ in 0..3 {
        e.backspace();
    }
    assert_eq!(e.text(), "make the kick lou");
    assert!(e.undo());
    assert_eq!((e.text().as_str(), e.cursor()), ("make the kick louder", 20));
    assert!(e.undo());
    assert_eq!((e.text().as_str(), e.cursor()), ("make the kick", 13));
    assert!(e.undo());
    assert_eq!((e.text().as_str(), e.cursor()), ("", 0));
    assert!(!e.undo());
    // Moving the cursor ends a run of typing, and so does deleting: each part comes back alone.
    typed(&mut e, "ab");
    e.left();
    typed(&mut e, "c");
    e.backspace();
    e.delete();
    assert_eq!(e.text(), "a");
    assert!(e.undo());
    assert_eq!(e.text(), "acb");
    assert!(e.undo());
    assert_eq!(e.text(), "ab");
    assert!(e.undo());
    assert_eq!(e.text(), "");
    // Replacing the text and erasing it are steps; once words are sent the next draft starts with nothing to undo.
    e.set("one");
    e.erase();
    assert!(e.undo());
    assert_eq!(e.text(), "one");
    e.clear();
    assert!(!e.undo());
}

#[test]
fn cuts_join_until_something_else_happens_and_ctrl_y_and_alt_y_bring_them_back() {
    let mut e = Editor::new();
    typed(&mut e, "one two three");
    // Two ctrl+w in a row are one cut, as in the shells, but two steps.
    e.delete_word_left();
    e.delete_word_left();
    assert_eq!(e.text(), "one ");
    assert!(e.yank());
    assert_eq!(e.text(), "one two three");
    // ctrl+u then ctrl+k from the middle of a line: one cut, read in order.
    e.word_left();
    e.kill_to_start();
    e.kill_to_end();
    assert_eq!(e.text(), "");
    typed(&mut e, "x ");
    assert!(e.yank());
    assert_eq!(e.text(), "x one two three");
    // alt+y swaps what ctrl+y put in for the cut before it, round to the newest again, and stays one step.
    assert!(e.yank_earlier());
    assert_eq!(e.text(), "x two three");
    assert!(e.yank_earlier());
    assert_eq!(e.text(), "x one two three");
    assert!(e.undo());
    assert_eq!(e.text(), "x ");
    // alt+y does nothing unless ctrl+y came just before.
    assert!(!e.yank_earlier());
    // A cut that cuts nothing changes nothing.
    e.end();
    e.kill_to_end();
    assert!(e.undo());
    assert_eq!(e.text(), "");
    // Ten cuts are kept: the eleventh pushes the first out.
    let mut e = Editor::new();
    for i in 0..11 {
        typed(&mut e, &format!("cut{i}"));
        e.kill_to_start();
        e.left();
    }
    assert!(e.yank());
    assert_eq!(e.text(), "cut10");
    for _ in 0..9 {
        assert!(e.yank_earlier());
    }
    assert_eq!(e.text(), "cut1");
    assert!(e.yank_earlier());
    assert_eq!(e.text(), "cut10");
}

#[test]
fn the_kill_ring_holds_at_most_four_megabytes_of_cuts() {
    let mut e = Editor::new();
    // 1.5 MB each, in three-byte characters, so there are fewer to segment.
    let long = "猫".repeat(500_000);
    for name in ["one", "two", "three"] {
        e.set(&format!("{name}{long}"));
        e.kill_to_start();
    }
    // Three cuts would hold 4.5 MB: the oldest goes.
    assert!(e.yank());
    assert!(e.text().starts_with("three"));
    assert!(e.yank_earlier());
    assert!(e.text().starts_with("two"));
    assert!(e.yank_earlier());
    assert!(e.text().starts_with("three"), "one was let go");
}

#[test]
fn a_draft_set_aside_for_an_earlier_message_comes_back_with_its_steps() {
    let mut e = Editor::new();
    typed(&mut e, "half a");
    e.left();
    let draft = e.show("an earlier message");
    assert_eq!((e.text().as_str(), e.cursor()), ("an earlier message", 18));
    // What's shown has no steps of its own; editing it makes some.
    assert!(!e.undo());
    typed(&mut e, "!");
    assert!(e.undo());
    assert_eq!(e.text(), "an earlier message");
    // What was cut while looking is still there to yank.
    e.kill_to_start();
    e.restore(draft);
    assert_eq!((e.text().as_str(), e.cursor()), ("half a", 5));
    assert!(e.undo());
    assert_eq!(e.text(), "");
    assert!(e.yank());
    assert_eq!(e.text(), "an earlier message");
}

/// What the editor was before it segmented only around each change: the whole text, segmented again every time.
struct Whole {
    text: String,
    cursor: usize,
}

impl Whole {
    fn offset(&self, at: usize) -> usize {
        graphemes(&self.text).iter().take(at).map(|g| g.len()).sum()
    }
    fn count(&self) -> usize {
        graphemes(&self.text).len()
    }
    fn replace(&mut self, from: usize, to: usize, text: &str) {
        let (start, end) = (self.offset(from), self.offset(to));
        self.text.replace_range(start..end, text);
        self.cursor = graphemes(&self.text[..start + text.len()]).len();
    }
}

#[test]
fn segmenting_around_each_change_matches_segmenting_the_whole_text() {
    // Characters that join their neighbours: marks, flags' letters, joined emoji and their skin tones, Hangul's
    // letters, Indic conjuncts, a prefix that joins what follows, and CR LF.
    let pieces = [
        "e", "\u{301}", "🇯", "🇵", "🇫", "🇷", "👨", "\u{200d}", "👩", "👍", "🏽", "\u{fe0f}", "\u{20e3}", "ᄀ", "ᅡ", "ᆨ", "가", "क", "\u{94d}",
        "ष", "\u{600}", "a", " ", "\n", "\r", "🏴",
    ];
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for _ in 0..40 {
        let mut e = Editor::new();
        let mut whole = Whole { text: String::new(), cursor: 0 };
        for step in 0..120 {
            match next(6) {
                0 | 1 => {
                    let text: String = (0..1 + next(3)).map(|_| pieces[next(pieces.len())]).collect();
                    e.insert(&text);
                    whole.replace(whole.cursor, whole.cursor, &text);
                }
                2 if whole.cursor > 0 => {
                    e.backspace();
                    whole.replace(whole.cursor - 1, whole.cursor, "");
                }
                3 if whole.cursor < whole.count() => {
                    e.delete();
                    let at = whole.cursor;
                    whole.replace(at, at + 1, "");
                }
                4 => {
                    e.left();
                    whole.cursor = whole.cursor.saturating_sub(1);
                }
                _ => {
                    e.right();
                    whole.cursor = (whole.cursor + 1).min(whole.count());
                }
            }
            assert_eq!((e.text(), e.cursor()), (whole.text.clone(), whole.cursor), "step {step}");
        }
    }
}

#[test]
fn typing_a_hundred_characters_after_a_long_paste_takes_less_than_a_frame() {
    let mut e = Editor::new();
    let paste = "the bass sits under the kick, a little to the left\n".repeat(1000);
    assert!(paste.len() > 50_000);
    e.insert(&paste);
    let typing: String =
        "and now a hundred characters typed one by one after it, as fast as anyone types them. ok!!!".chars().cycle().take(100).collect();
    let started = Instant::now();
    typed(&mut e, &typing);
    let took = started.elapsed();
    assert!(took < Duration::from_millis(16), "{took:?}");
    assert_eq!(e.text(), paste + &typing);
    // All of it is one step.
    assert!(e.undo());
    assert!(e.text().ends_with("to the left\n"));
}

case!(undo_yank_and_ctrl_c_in_the_composer, async {
    use kumi::history::open_input_history;
    use std::cell::RefCell;
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("input-history");
    let history = Rc::new(RefCell::new(open_input_history(Some(file.clone()), vec![])));
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.history = Some(history));
    h.start().await;
    h.connect();
    h.type_text("widen the pad").await;
    h.type_text("\x1b[200~ and the strings\x1b[201~").await;
    h.has("widen the pad and the strings");
    // ctrl+- as legacy terminals send it (ctrl+_), then as the kitty protocol does.
    h.type_text("\x1f").await;
    h.has("widen the pad");
    assert!(!has(&h.screen(), "and the strings"));
    h.type_text("\x1b[45;5u").await;
    assert!(!has(&h.screen(), "widen the pad"));
    // ctrl+w twice is one cut; after a move, ctrl+k makes another. ctrl+y puts the newest back, alt+y the one before.
    h.type_text("reverb on the snare").await;
    h.type_text("\x17\x17").await;
    h.has("reverb on");
    assert!(!has(&h.screen(), "the snare"));
    h.type_text("\x01\x0b").await;
    assert!(!has(&h.screen(), "reverb on"));
    h.type_text("more ").await;
    h.type_text("\x19").await;
    h.has("more reverb on");
    h.type_text("\x1by").await;
    h.has("more the snare");
    // ctrl+c on a draft keeps it: ↑ brings it back, and so does ctrl+-.
    h.type_text("\x03").await;
    assert!(!has(&h.screen(), "more the snare"));
    assert!(!h.calls().contains(&"close".into()));
    // Kept while Kumi runs, never written to the history's file.
    assert!(!std::fs::read_to_string(&file).unwrap_or_default().contains("more the snare"));
    h.type_text("\x1b[A").await;
    h.has("more the snare");
    h.type_text("\x1b[B").await;
    assert!(!has(&h.screen(), "more the snare"));
    h.type_text("\x1f").await;
    h.has("more the snare");
    h.type_text("\r").await;
    assert!(h.calls().contains(&"submit:more the snare".into()), "{:?}", h.calls());
    // Looking back sets the draft aside: an edit to an earlier message is undone, and ↓ brings the draft back.
    let composer = |h: &Harness| {
        let lines = h.screen();
        lines[lines.len() - 4..].to_vec()
    };
    h.type_text("a new draft").await;
    h.type_text("\x1b[A").await;
    assert!(has(&composer(&h), "more the snare"));
    h.type_text(" now").await;
    assert!(has(&composer(&h), "more the snare now"));
    h.type_text("\x1f").await;
    assert!(has(&composer(&h), "more the snare") && !has(&composer(&h), "snare now"));
    h.type_text("\x1b[B").await;
    assert!(has(&composer(&h), "a new draft"));
    h.close().await;
});
