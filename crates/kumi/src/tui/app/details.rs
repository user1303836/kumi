//! ctrl+o: the conversation in detail, over the conversation. The session gives it as the full export would show it
//! (asked once, as the view opens), and the turn still under way comes from Kumi's own transcript. `/` searches it,
//! `{` and `}` go from request to request, `[` prints it into the terminal's scrollback, and `v` opens it in the
//! producer's editor.

use super::super::{
    details::Details,
    external_editor,
    keys::Modifiers,
    overlay::{Area, Handled, Overlay},
    render::Cursor,
};
use super::drawing::st;
use super::*;
use crate::export::redactor_now;

/// The details as an overlay: None while the session's answer is on its way.
pub(super) struct DetailsPanel {
    details: Option<Details>,
    /// The full export as Markdown, for the editor.
    markdown: String,
    /// What's typed goes into the search.
    searching: bool,
    /// The last frame's text width and rows, for the keys between frames.
    width: i32,
    height: usize,
}

impl TuiApp {
    /// Opens the details, and asks the session for the conversation to show in them.
    pub(super) fn open_details(&self) {
        let under_way: Vec<String> = {
            let state = self.0.state.borrow();
            let current = state.current.as_ref().map(|entry| entry.borrow().clone());
            match current {
                Some(Entry::Assistant { steps, status: AnswerStatus::Running, .. }) => steps
                    .iter()
                    .map(|step| match step.doing.as_deref().filter(|doing| !doing.is_empty()) {
                        Some(doing) => format!("{} · {doing}", step.label),
                        None => step.label.clone(),
                    })
                    .collect(),
                _ => vec![],
            }
        };
        let panel = self.open_overlay(DetailsPanel { details: None, markdown: String::new(), searching: false, width: 60, height: 20 });
        self.task(move |app| async move {
            let secrets = app.0.state.borrow().secrets.clone();
            let redact = redactor_now(app.0.options.redact.as_ref(), &secrets).await;
            let found = app.session().op(op::ConversationToExport {}).await.ok().flatten();
            let (parts, markdown) = crate::export::details_now(found, &*redact).unwrap_or_default();
            let mut panel = panel.borrow_mut();
            panel.details = Some(Details::new(parts, under_way));
            panel.markdown = markdown;
            drop(panel);
            app.0.scheduler.request();
            Ok(())
        });
    }

    /// `[`: the details in the terminal's scrollback, until a key brings Kumi's screen back.
    fn print_details(&self, text: String) {
        self.task(move |app| async move {
            app.0
                .tty
                .hand_over(async {
                    // Kumi's own writes wait while the terminal is lent; this one is meant for it.
                    app.0.options.output.write(&format!("{text}\n— Any key goes back to Kumi —\n"));
                    app.0.tty.wait_for_key().await;
                })
                .await;
            app.taken_back();
            Ok(())
        });
    }

    /// `v`: the full export in the producer's editor. Nothing comes back from it, and its file stays until Kumi quits, in
    /// case the editor opens a window and returns at once.
    fn edit_details(&self, markdown: String) {
        let Some(editor) = self.0.options.editor.clone() else {
            self.notice("Set VISUAL or EDITOR to your editor (export EDITOR=nano, say), and v opens the details in it.", NoticeTone::Info);
            return;
        };
        let file = tempfile::Builder::new().prefix("kumi-details-").suffix(".md").tempfile().and_then(|file| {
            std::fs::write(file.path(), &markdown)?;
            Ok(file.into_temp_path())
        });
        let file = match file {
            Ok(file) => file,
            Err(error) => {
                self.notice(&format!("Kumi couldn't make a file for the details: {error}"), NoticeTone::Warn);
                return;
            }
        };
        let path = file.to_path_buf();
        self.0.state.borrow_mut().details_files.push(file);
        self.task(move |app| async move {
            let run = app.0.options.run_editor.clone();
            let _ = app
                .0
                .tty
                .hand_over(async {
                    match run {
                        Some(run) => run(path.clone()).await,
                        None => external_editor::run(&editor, &path).await.map(|status| status.code().unwrap_or(-1)),
                    }
                })
                .await;
            app.taken_back();
            Ok(())
        });
    }
}

impl DetailsPanel {
    /// A key typed: into the search while it's being typed; otherwise the view's own (true when it closes the view).
    fn typed(&mut self, app: &TuiApp, typed: char) -> bool {
        let (width, height) = (self.width, self.height);
        let Some(details) = self.details.as_mut() else { return typed == 'q' };
        if self.searching {
            let query = format!("{}{typed}", details.query);
            details.search(&query, width);
            details.show_match(width, height);
            return false;
        }
        match typed {
            'q' => return true,
            '/' => {
                self.searching = true;
                details.search("", width);
            }
            'n' | 'N' => details.next_match(typed == 'n', width, height),
            '}' | '{' => details.next_request(typed == '}', width),
            '[' => app.print_details(details.printed(app.0.tty.size().columns - 1)),
            'v' => app.edit_details(self.markdown.clone()),
            _ => {}
        }
        false
    }

    /// Backspace, enter and esc while the search is typed: enter keeps it, esc drops it.
    fn search_key(&mut self, name: &str) {
        let (width, height) = (self.width, self.height);
        let Some(details) = self.details.as_mut() else { return };
        let mut query = details.query.clone();
        match name {
            "backspace" => {
                query.pop();
            }
            "enter" => {
                self.searching = false;
                return;
            }
            _ => {
                self.searching = false;
                query.clear();
            }
        }
        details.search(&query, width);
        details.show_match(width, height);
    }

    /// The arrows, the pages, home and end.
    fn scroll_key(&mut self, name: &str) {
        let (width, height) = (self.width, self.height);
        let Some(details) = self.details.as_mut() else { return };
        let page = height.saturating_sub(1).max(1) as i32;
        let rows = match name {
            "up" => -1,
            "down" => 1,
            "pageup" => -page,
            "pagedown" => page,
            "home" => i32::MIN / 2,
            "end" => i32::MAX / 2,
            _ => return,
        };
        details.scroll(rows, width, height);
    }
}

impl Overlay<TuiApp> for DetailsPanel {
    /// The view's keys; while searching, what's typed goes into the search (enter keeps it, esc drops it).
    fn handle(&mut self, app: &TuiApp, event: InputEvent) -> Handled {
        let closing = |name: &str, mods: Modifiers| name == "escape" || (mods.ctrl && matches!(name, "c" | "o"));
        match event {
            InputEvent::Key { ref name, .. } if self.searching && matches!(name.as_str(), "backspace" | "enter" | "escape") => {
                self.search_key(name);
                Handled::Kept
            }
            InputEvent::Key { ref name, mods, .. } if closing(name, mods) => Handled::Close,
            InputEvent::Key { ref name, .. } => {
                self.scroll_key(name);
                Handled::Kept
            }
            // Typed fast, or by a test, a run of keys comes as one text: each is taken in turn.
            InputEvent::Text { ref text } => {
                for typed in text.chars() {
                    if self.typed(app, typed) {
                        return Handled::Close;
                    }
                }
                Handled::Kept
            }
            InputEvent::Paste { ref text } if self.searching => {
                for typed in sanitize_text(text, &[]).replace('\n', " ").chars() {
                    self.typed(app, typed);
                }
                Handled::Kept
            }
            _ => Handled::Kept,
        }
    }

    /// In the conversation's place, from under the header down to the box.
    fn draw(&mut self, app: &TuiApp, screen: &mut Screen, area: Area) -> Option<Cursor> {
        let rect = Rect::new(0, 2, area.left, (area.box_top - 3).max(1));
        // What's under it can't be clicked, dragged over to copy, or scrolled by the wheel through it.
        app.cover(rect);
        self.width = (rect.width - 5).max(1);
        self.height = (rect.height - i32::from(self.details.as_ref().is_some_and(|details| !details.query.is_empty()))).max(1) as usize;
        match self.details.as_mut() {
            Some(details) => details.draw(screen, rect),
            None => {
                screen.fill(rect, &st::GROUND);
                screen.put(rect.x + 3, rect.y + 1, "Reading the conversation…", &st::FAINT);
            }
        }
        None
    }

    /// Short enough for the box's row, which leaves out a hint that doesn't fit.
    fn hint(&self, _: &TuiApp) -> String {
        if self.searching {
            "type to search · enter keeps it · esc drops it".into()
        } else if self.details.as_ref().is_some_and(|details| !details.query.is_empty()) {
            "n N matches · / searches again · { } requests · q closes".into()
        } else {
            "/ search · { } requests · [ scrollback · v editor · q closes".into()
        }
    }

    fn scroll(&mut self, _: &TuiApp, rows: i32) -> bool {
        let (width, height) = (self.width, self.height);
        if let Some(details) = self.details.as_mut() {
            details.scroll(rows, width, height);
        }
        true
    }
}
