//! The composer's ways beyond typing: looking back through what was sent for some words (ctrl+r), the draft in the
//! producer's own editor (ctrl+g), and a draft put aside (ctrl+s).

use super::super::{
    editor::{Draft, Editor, EditorLayout},
    external_editor,
    width::{graphemes, text_width},
};
use super::drawing::st;
use super::*;
use std::time::{Duration, Instant};

/// ctrl+r: the words looked for, and the earlier message found with them, by its place in the input history.
pub(super) struct Search {
    query: String,
    found: Option<usize>,
}

/// ctrl+s: the draft put aside, with its undo steps and the files that were to go with it.
pub(super) struct Stash {
    draft: Draft,
    attachments: Vec<Attachment>,
}

const LOOKING: &str = "search: ";
pub(super) const SEARCH_HINT: &[(&[&str], &str)] =
    &[(&["search.back"], "older"), (&["search.take"], "uses it"), (&["search.cancel"], "goes back")];
pub(super) const STASH_HINT: &[(&[&str], &str)] = &[(&["composer.stash"], "brings back what you put aside")];

/// An editor that closes sooner than this, leaving the draft unchanged, likely opened a window and didn't wait for it.
const AT_ONCE: Duration = Duration::from_secs(1);

/// The newest message before `before` with `query` in it, in any case, other than `skip` (the one shown already).
fn find(entries: &[String], query: &str, before: usize, skip: Option<&str>) -> Option<usize> {
    let query = query.to_lowercase();
    (0..before.min(entries.len())).rev().find(|&at| Some(entries[at].as_str()) != skip && entries[at].to_lowercase().contains(&query))
}

/// Looking back through what was sent ends, as ↓ past the newest ends it: the draft set aside comes back.
fn back_to_draft(state: &mut State) {
    if let Some((_, draft)) = state.recall.take() {
        state.editor.restore(draft);
    }
}

impl TuiApp {
    /// What was sent, oldest first, for `look` to go through without copying it.
    fn with_sent<T>(&self, look: impl FnOnce(&[String]) -> T) -> T {
        match &self.0.options.history {
            Some(history) => look(history.borrow().entries()),
            None => look(&[]),
        }
    }

    pub(super) fn searching(&self) -> bool {
        self.0.state.borrow().search.is_some()
    }

    /// ctrl+r: starts looking back through what was sent, newest first; while looking, the next earlier message with the
    /// words typed. The draft stays as it is until a message found is used.
    pub(super) fn search_back(&self) {
        let state = &mut *self.0.state.borrow_mut();
        self.with_sent(|sent| match &mut state.search {
            Some(search) => {
                if let Some(at) = search.found {
                    search.found = find(sent, &search.query, at, Some(&sent[at])).or(Some(at));
                }
            }
            None => state.search = Some(Search { query: String::new(), found: find(sent, "", sent.len(), None) }),
        });
    }

    /// Typed or pasted while looking: the words to find, found again from the newest.
    pub(super) fn search_type(&self, text: &str) {
        if let Some(search) = &mut self.0.state.borrow_mut().search {
            search.query.push_str(text);
            search.found = self.with_sent(|sent| find(sent, &search.query, sent.len(), None));
        }
    }

    /// Takes back the last character looked for, and finds again from the newest.
    pub(super) fn search_erase(&self) {
        if let Some(search) = &mut self.0.state.borrow_mut().search {
            let mut letters = graphemes(&search.query);
            letters.pop();
            search.query = letters.concat();
            search.found = self.with_sent(|sent| find(sent, &search.query, sent.len(), None));
        }
    }

    /// Stops looking. `take`: the message found goes into the box, one step ctrl+- takes back; else the draft stays.
    pub(super) fn search_end(&self, take: bool) {
        let state = &mut *self.0.state.borrow_mut();
        let Some(search) = state.search.take() else { return };
        if let Some(found) = search.found.filter(|_| take).and_then(|at| self.with_sent(|sent| sent.get(at).cloned())) {
            back_to_draft(state);
            // A message sent with chips this run goes in with them.
            match state.sent_words.iter().rev().find(|(kept, _)| *kept == found).map(|(_, words)| words.clone()) {
                Some(words) => state.editor.set_composed(&words),
                None => state.editor.set(&found),
            }
            state.menu_dismissed = true;
        }
    }

    /// The box's rows: the draft's, or, while looking, what's looked for (the cursor at its end, where an input method
    /// opens) and the message found.
    pub(super) fn composer_layout(&self, width: i32) -> EditorLayout {
        let state = self.0.state.borrow();
        let Some(search) = &state.search else {
            return state.editor.layout(width);
        };
        // The end of a long query, where the cursor is.
        let mut query = graphemes(&search.query);
        while !query.is_empty() && text_width(LOOKING) + text_width(&query.concat()) >= width {
            query.remove(0);
        }
        let first = format!("{LOOKING}{}", query.concat());
        let mut rows = vec![first.clone()];
        match search.found.and_then(|at| self.with_sent(|sent| sent.get(at).cloned())) {
            Some(found) => {
                let mut shown = Editor::new();
                match state.sent_words.iter().rev().find(|(kept, _)| *kept == found) {
                    Some((_, words)) => shown.set_composed(words),
                    None => shown.set(&found),
                }
                rows.extend(shown.layout(width).rows);
            }
            None => rows.push(if search.query.is_empty() { "Nothing sent yet".into() } else { "Nothing sent has these words".into() }),
        }
        EditorLayout { cursor_row: 0, cursor_column: text_width(&first), rows }
    }

    /// Draws the box's rows while looking: the words looked for, then the message found, or that there's none.
    pub(super) fn draw_search(&self, screen: &mut Screen, x: i32, y: i32, rows: &[String], box_: Rect) {
        let found = self.0.state.borrow().search.as_ref().is_some_and(|search| search.found.is_some());
        for (index, row) in rows.iter().enumerate() {
            let y = y + index as i32;
            if index == 0 {
                screen.put_in(x, y, LOOKING, &st::DIM, box_);
                screen.put_in(x + text_width(LOOKING), y, row.strip_prefix(LOOKING).unwrap_or(row), &st::BRIGHT, box_);
            } else {
                screen.put_in(x, y, row, if found { &st::TEXT } else { &st::FAINT }, box_);
            }
        }
    }

    /// ctrl+g: the draft in the producer's own editor, on a private file that goes once it's read back. The terminal is
    /// lent to the editor meanwhile. What it saved comes back as the draft, one step ctrl+- takes back.
    pub(super) fn edit_draft(&self) {
        let Some(editor) = self.0.options.editor.clone() else {
            self.notice(
                &self.key_words(
                    "composer.editor",
                    "Set VISUAL or EDITOR to your editor (export EDITOR=nano, say), and {} opens the draft in it.",
                    "Set VISUAL or EDITOR to your editor (export EDITOR=nano, say).",
                ),
                NoticeTone::Info,
            );
            return;
        };
        // The editor gets each chip's whole paste.
        let draft = self.0.state.borrow().editor.expanded();
        let file = tempfile::Builder::new().prefix("kumi-draft-").suffix(".md").tempfile().and_then(|file| {
            std::fs::write(file.path(), &draft)?;
            Ok(file)
        });
        let file = match file {
            Ok(file) => file,
            Err(error) => {
                self.notice(&format!("Kumi couldn't make a file for the draft: {error}"), NoticeTone::Warn);
                return;
            }
        };
        self.task(move |app| async move {
            let path = file.path().to_path_buf();
            let run = app.0.options.run_editor.clone();
            let started = Instant::now();
            let closed = app
                .0
                .tty
                .hand_over(async {
                    match run {
                        Some(run) => run(path.clone()).await,
                        None => external_editor::run(&editor, &path).await.map(|status| status.code().unwrap_or(-1)),
                    }
                })
                .await;
            let at_once = started.elapsed() < AT_ONCE;
            let saved = std::fs::read_to_string(&path);
            drop(file);
            app.taken_back();
            match (closed, saved) {
                (Ok(0), Ok(saved)) if at_once && saved == draft => app.notice(
                    &format!("{editor} closed at once, so the draft is as it was. An editor that opens a window needs to wait for it: code --wait, say."),
                    NoticeTone::Info,
                ),
                (Ok(0), Ok(saved)) => {
                    // An editor ends a file with a new line; the draft didn't have one.
                    let saved = saved.strip_suffix('\n').map(|text| text.strip_suffix('\r').unwrap_or(text)).unwrap_or(&saved);
                    let named = {
                        let state = &mut *app.0.state.borrow_mut();
                        let named = !state.editor.references().is_empty();
                        back_to_draft(state);
                        state.editor.set(&sanitize_text(saved, &[]));
                        state.menu_dismissed = true;
                        named
                    };
                    // What the draft named with @ went to the editor as words, and comes back so.
                    if named {
                        app.notice("What you named with @ came back from the editor as words: name it again with @ to send it exactly.", NoticeTone::Info);
                    }
                }
                (Ok(0), Err(error)) => app.notice(&format!("Kumi couldn't read the draft back: {error}"), NoticeTone::Warn),
                // The shell's words for a command it can't find, or can't run.
                (Ok(127 | 126), _) => app.notice(
                    &format!("Kumi couldn't run {editor}. Set VISUAL or EDITOR to an editor this computer has (export EDITOR=nano, say)."),
                    NoticeTone::Warn,
                ),
                (Ok(_), _) => app.notice(&format!("{editor} ended with an error, so the draft is as it was."), NoticeTone::Info),
                (Err(error), _) => app.notice(&format!("Kumi couldn't open {editor}: {error}"), NoticeTone::Warn),
            }
            app.0.scheduler.request();
            Ok(())
        });
    }

    /// Taken back from the producer's editor: all of the screen is drawn again. Pictures go up again too, as a terminal
    /// may drop them with the screen they were on: those up are taken down, and what's shown is uploaded afresh.
    pub(super) fn taken_back(&self) {
        self.redraw_pictures();
        self.0.renderer.borrow_mut().invalidate();
        self.0.state.borrow_mut().status_sent = None;
    }

    /// ctrl+s: puts the draft aside with its files, or, in an empty box, brings back what was put aside. When there's
    /// both, they change places, so nothing is lost. Looking back through what was sent ends first, as ↓ would end it.
    pub(super) fn stash(&self) {
        let state = &mut *self.0.state.borrow_mut();
        if let Some((_, draft)) = state.recall.take() {
            state.editor.restore(draft);
        }
        let empty = state.editor.is_empty() && state.attachments.is_empty();
        let put = (!empty).then(|| Stash { draft: state.editor.show(""), attachments: std::mem::take(&mut state.attachments) });
        if let Some(Stash { draft, attachments }) = state.stash.take() {
            state.editor.restore(draft);
            state.attachments = attachments;
        }
        state.stash = put;
        state.menu_dismissed = true;
    }
}
