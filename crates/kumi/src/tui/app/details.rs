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
                    app.0.options.output.write(&format!("{text}\n— Any key goes back to Kumi —"));
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

impl Overlay<TuiApp> for DetailsPanel {
    /// The view's keys; while searching, what's typed goes into the search (enter keeps it, esc drops it).
    fn handle(&mut self, app: &TuiApp, event: InputEvent) -> Handled {
        let (width, height) = (self.width, self.height);
        let closing = |name: &str, mods: Modifiers| name == "escape" || (mods.ctrl && matches!(name, "c" | "o"));
        let Some(details) = self.details.as_mut() else {
            return match event {
                InputEvent::Key { ref name, mods, .. } if closing(name, mods) => Handled::Close,
                InputEvent::Text { ref text } if text == "q" => Handled::Close,
                _ => Handled::Kept,
            };
        };
        if self.searching {
            let mut query = details.query.clone();
            match event {
                InputEvent::Text { ref text } | InputEvent::Paste { ref text } => {
                    query.push_str(&sanitize_text(text, &[]).replace('\n', " "))
                }
                InputEvent::Key { ref name, .. } if name == "backspace" => {
                    query.pop();
                }
                InputEvent::Key { ref name, .. } if name == "enter" => {
                    self.searching = false;
                    return Handled::Kept;
                }
                InputEvent::Key { ref name, .. } if name == "escape" => {
                    self.searching = false;
                    query.clear();
                }
                _ => return Handled::Kept,
            }
            details.search(&query, width);
            details.show_match(width, height);
            return Handled::Kept;
        }
        match event {
            InputEvent::Key { ref name, mods, .. } if closing(name, mods) => return Handled::Close,
            InputEvent::Key { ref name, .. } => {
                let page = height.saturating_sub(1).max(1) as i32;
                let rows = match name.as_str() {
                    "up" => -1,
                    "down" => 1,
                    "pageup" => -page,
                    "pagedown" => page,
                    "home" => i32::MIN / 2,
                    "end" => i32::MAX / 2,
                    _ => 0,
                };
                details.scroll(rows, width, height);
            }
            InputEvent::Text { ref text } => match text.as_str() {
                "q" => return Handled::Close,
                "/" => {
                    self.searching = true;
                    details.search("", width);
                }
                "n" | "N" => details.next_match(text == "n", width, height),
                "}" | "{" => details.next_request(text == "}", width),
                "[" => app.print_details(details.printed(app.0.tty.size().columns - 1)),
                "v" => app.edit_details(self.markdown.clone()),
                _ => {}
            },
            _ => {}
        }
        Handled::Kept
    }

    /// In the conversation's place, from under the header down to the box.
    fn draw(&mut self, _: &TuiApp, screen: &mut Screen, area: Area) -> Option<Cursor> {
        let rect = Rect::new(0, 2, area.left, (area.box_top - 3).max(1));
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

    fn hint(&self, _: &TuiApp) -> String {
        if self.searching {
            "type to search · enter keeps it · esc drops it".into()
        } else {
            "/ search · n N matches · { } requests · [ scrollback · v editor · q closes".into()
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
