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

/// A key typed after a long paste costs what one typed after a paste a hundred times shorter does: the editor segments
/// around the edit, not the whole text again (which made a key after 50 KB take a frame). Measured as the fastest of
/// rounds taken by turns, so a busy machine slows both alike and can't fail it, with no clock bound to miss (#646).
#[test]
fn typing_after_a_long_paste_costs_what_typing_after_a_short_one_does() {
    let line = "the bass sits under the kick, a little to the left\n";
    let (short_paste, long_paste) = (line.repeat(10), line.repeat(1000));
    assert!(long_paste.len() > 50_000);
    let (mut short, mut long) = (Editor::new(), Editor::new());
    short.insert(&short_paste);
    long.insert(&long_paste);
    // The run's first key keeps the text for undo, once, as a run of typing does: the keys after it are what's timed.
    typed(&mut short, "a");
    typed(&mut long, "a");
    let (mut fastest_short, mut fastest_long) = (Duration::MAX, Duration::MAX);
    for _ in 0..20 {
        for (editor, fastest) in [(&mut short, &mut fastest_short), (&mut long, &mut fastest_long)] {
            let started = Instant::now();
            typed(editor, "punch");
            *fastest = (*fastest).min(started.elapsed());
        }
    }
    let after = format!("a{}", "punch".repeat(20));
    assert!(
        fastest_long < fastest_short * 10 + Duration::from_micros(200),
        "five keys took {fastest_long:?} after the long paste, {fastest_short:?} after the short one"
    );
    assert_eq!(long.text(), long_paste.clone() + &after);
    // All of it is one step.
    assert!(long.undo());
    assert_eq!(long.text(), long_paste);
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

/// The input box's rows at the bottom of a 120 × 36 screen.
fn composer(h: &Harness) -> Vec<String> {
    let lines = h.screen();
    lines[lines.len() - 4..].to_vec()
}

fn with_history(entries: &[&str]) -> (tempfile::TempDir, Rc<std::cell::RefCell<kumi::history::InputHistory>>) {
    let tmp = tempfile::tempdir().unwrap();
    let mut history = kumi::history::open_input_history(Some(tmp.path().join("input-history")), vec![]);
    for entry in entries {
        history.add(entry);
    }
    (tmp, Rc::new(std::cell::RefCell::new(history)))
}

case!(ctrl_r_finds_what_was_sent_and_esc_keeps_the_draft, async {
    let (_tmp, history) = with_history(&["make the kick punchier", "add reverb to the snare", "make the kick shorter"]);
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.history = Some(history));
    h.start().await;
    h.connect();
    h.type_text("my draft").await;
    // Looking starts at the newest; words narrow it; ctrl+r again goes older, and stays on the oldest found.
    h.type_text("\x12").await;
    assert!(has(&composer(&h), "search:") && has(&composer(&h), "make the kick shorter"));
    h.has("ctrl+r older · enter uses it · esc goes back");
    h.type_text("kick").await;
    assert!(has(&composer(&h), "search: kick") && has(&composer(&h), "make the kick shorter"));
    h.type_text("\x12").await;
    assert!(has(&composer(&h), "make the kick punchier"));
    h.type_text("\x12").await;
    assert!(has(&composer(&h), "make the kick punchier"));
    h.type_text("zz").await;
    assert!(has(&composer(&h), "Nothing sent has these words"));
    h.type_text("\x7f\x7f").await;
    assert!(has(&composer(&h), "make the kick shorter"));
    // esc goes back to the draft as it was.
    h.type_text("\x1b").await;
    h.wait_until_hidden("search:").await;
    assert!(has(&composer(&h), "my draft"));
    // enter puts what's found in the box, a step ctrl+- takes back; nothing is sent.
    h.type_text("\x12").await;
    h.type_text("snare").await;
    h.type_text("\r").await;
    assert!(has(&composer(&h), "add reverb to the snare") && !has(&composer(&h), "search:"));
    assert!(!h.calls().iter().any(|call| call.starts_with("submit")));
    h.type_text("\x1f").await;
    assert!(has(&composer(&h), "my draft"));
    // A command looked for doesn't open the menu while looking.
    h.type_text("\x15/").await;
    h.type_text("\x12").await;
    assert!(!has(&h.screen(), "Start a fresh conversation"));
    h.close().await;
});

case!(ctrl_g_lends_the_terminal_to_the_editor_and_takes_the_draft_back, async {
    use futures::FutureExt;
    use std::{cell::RefCell, path::PathBuf};
    let seen = Rc::new(RefCell::new(Vec::<(PathBuf, String)>::new()));
    let works = Rc::new(std::cell::Cell::new(true));
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| {
        o.editor = Some("fake-editor".into());
        let (seen, works) = (seen.clone(), works.clone());
        o.run_editor = Some(Rc::new(move |path: PathBuf| {
            let (seen, works) = (seen.clone(), works.get());
            async move {
                seen.borrow_mut().push((path.clone(), std::fs::read_to_string(&path)?));
                std::fs::write(&path, "tighten the hats\nand the snare\n")?;
                Ok(works)
            }
            .boxed_local()
        }));
    });
    h.start().await;
    h.connect();
    h.type_text("tighten").await;
    let before = h.output.text.borrow().len();
    h.type_text("\x07").await;
    h.wait_for("and the snare").await;
    assert!(has(&composer(&h), "tighten the hats"));
    // The editor had the draft, on a file that's gone once it's read back.
    let (path, draft) = seen.borrow()[0].clone();
    assert_eq!(draft, "tighten");
    assert!(!path.exists());
    // The terminal went back as Kumi found it while the editor had it, then Kumi took it again.
    let written = h.output.text.borrow()[before..].to_string();
    let lent = written.find(kumi::tui::tty::RESTORE).expect("given back");
    assert!(written[lent..].contains("\x1b[?1049h"));
    // What came back is one step: ctrl+- brings the draft back.
    h.type_text("\x1f").await;
    assert!(has(&composer(&h), "tighten") && !has(&composer(&h), "and the snare"));
    // An editor that ends with an error leaves the draft as it was.
    works.set(false);
    h.type_text("\x07").await;
    h.wait_for("ended with an error").await;
    assert!(has(&composer(&h), "tighten") && !has(&composer(&h), "and the snare"));
    h.close().await;
});

case!(ctrl_g_without_an_editor_says_how_to_choose_one, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("a draft\x07").await;
    h.has("Set VISUAL or EDITOR to your editor");
    h.close().await;
});

case!(ctrl_s_puts_the_draft_aside_with_its_files_and_brings_it_back, async {
    let dir = tempfile::tempdir().unwrap();
    let picture = dir.path().join("Screen Shot.png");
    std::fs::write(&picture, vec![1u8; 1500]).unwrap();
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text(&format!("\x1b[200~'{}'\x1b[201~", picture.display())).await;
    h.type_text("half a thought").await;
    h.has("with Screen Shot.png");
    h.type_text("\x13").await;
    assert!(!has(&composer(&h), "half a thought"));
    assert!(!has(&h.screen(), "with Screen Shot.png"));
    h.has("ctrl+s brings back what you put aside");
    // With words in the box, the two change places.
    h.type_text("something else").await;
    h.type_text("\x13").await;
    assert!(has(&composer(&h), "half a thought"));
    h.has("with Screen Shot.png");
    h.type_text("\x15\x7f").await;
    assert!(!has(&h.screen(), "with Screen Shot.png"));
    h.type_text("\x13").await;
    assert!(has(&composer(&h), "something else"));
    // The draft comes back with its steps.
    h.type_text("\x1f").await;
    assert!(!has(&composer(&h), "something else"));
    h.close().await;
});

#[test]
fn the_editor_is_visual_then_editor_and_found_the_way_windows_finds_programs() {
    use kumi::tui::external_editor::{editor_command, resolve, words};
    use std::path::PathBuf;
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string())
    };
    assert_eq!(editor_command(env(&[("VISUAL", "code --wait"), ("EDITOR", "vim")]), false).as_deref(), Some("code --wait"));
    assert_eq!(editor_command(env(&[("VISUAL", " "), ("EDITOR", "nano")]), false).as_deref(), Some("nano"));
    assert_eq!(editor_command(env(&[]), false), None);
    assert_eq!(editor_command(env(&[]), true).as_deref(), Some("notepad"));
    assert_eq!(
        words(r#""C:\Program Files\Notepad++\notepad++.exe" -multiInst  -nosession"#),
        [r"C:\Program Files\Notepad++\notepad++.exe", "-multiInst", "-nosession"]
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("code.cmd"), "").unwrap();
    let path = format!("{};{}", dir.path().join("nothing here").display(), dir.path().display());
    assert_eq!(resolve("code", &path, ".COM;.EXE;.BAT;.CMD"), dir.path().join("code.cmd"));
    assert_eq!(resolve("notepad.exe", &path, ".EXE"), PathBuf::from("notepad.exe"));
    assert_eq!(resolve("vim", &path, ".EXE"), PathBuf::from("vim"));
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn the_editor_runs_through_the_shell_with_its_arguments_and_the_file_last() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    // A space in its name, quoted in the command, as it would be in EDITOR.
    let script = dir.path().join("my editor");
    std::fs::write(&script, "#!/bin/sh\nprintf '%s then' \"$1\" > \"$2\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let file = dir.path().join("draft.md");
    std::fs::write(&file, "").unwrap();
    let status = kumi::tui::external_editor::run(&format!("'{}' tighten", script.display()), &file).await.unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "tighten then");
}

#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn the_editor_is_found_through_pathext_and_runs_on_the_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fake-editor.cmd"), "@echo edited> %1\r\n").unwrap();
    // Each test runs in a process of its own: the PATH it finds the editor on is this one's.
    std::env::set_var("PATH", format!("{};{}", dir.path().display(), std::env::var("PATH").unwrap_or_default()));
    let file = dir.path().join("draft.md");
    std::fs::write(&file, "").unwrap();
    let status = kumi::tui::external_editor::run("fake-editor", &file).await.unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "edited");
}
