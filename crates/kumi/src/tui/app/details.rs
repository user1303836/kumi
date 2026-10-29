//! ctrl+o: the conversation in detail, over the conversation. The session gives it as the full export would show it
//! (asked once, as the view opens), and the turn still under way comes from Kumi's own transcript. `/` searches it,
//! `{` and `}` go from request to request, `[` prints it into the terminal's scrollback, and `v` opens it in the
//! producer's editor.

use super::super::{
    details::{words_of, AnswerMeta, Details},
    external_editor,
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
        let metas = self.answer_metas();
        let panel = self.open_overlay(DetailsPanel { details: None, markdown: String::new(), searching: false, width: 60, height: 20 });
        self.task(move |app| async move {
            let secrets = app.0.state.borrow().secrets.clone();
            let redact = redactor_now(app.0.options.redact.as_ref(), &secrets).await;
            let found = app.session().op(op::ConversationToExport {}).await.ok().flatten();
            let (parts, markdown) = crate::export::details_now(found, &*redact).unwrap_or_default();
            let mut panel = panel.borrow_mut();
            panel.details = Some(Details::new(parts, under_way, metas));
            panel.markdown = markdown;
            drop(panel);
            app.0.scheduler.request();
            Ok(())
        });
    }

    /// The finished answers this transcript has, oldest first: each one's opening words, and its line for the details (the
    /// model that gave it, when it started, and its own quiet line of time, changes and cost).
    fn answer_metas(&self) -> Vec<AnswerMeta> {
        let state = self.0.state.borrow();
        let (now, wall) = (perf_now(), kumi_common::time::now_ms() as f64);
        state
            .transcript
            .entries
            .iter()
            .filter_map(|entry| {
                let Entry::Assistant { text, status, started_at, footer, .. } = &*entry.borrow() else { return None };
                if text.trim().is_empty() || *status == AnswerStatus::Running {
                    return None;
                }
                let started = started_at.and_then(|at| {
                    use chrono::TimeZone;
                    chrono::Local.timestamp_millis_opt((wall - (now - at)) as i64).single().map(|at| at.format("%H:%M").to_string())
                });
                let line = [state.answer_models.get(&entry.id).cloned(), started, footer.clone()].into_iter().flatten().collect::<Vec<_>>();
                Some(AnswerMeta { words: words_of(text), line: line.join(" · ") })
            })
            .collect()
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
    /// Where its keys are read: while a search is typed, backspace, enter and esc are the search's.
    pub(super) fn scope(&self) -> Scope {
        if self.searching {
            Scope::DetailsSearch
        } else {
            Scope::Details
        }
    }

    /// Does what the first of a key's actions (`ids`) that applies does: Some(true) when that closes the view, None when
    /// none applied. With nothing to show yet, only closing applies.
    fn act(&mut self, app: &TuiApp, ids: Vec<&'static str>) -> Option<bool> {
        let (width, height) = (self.width, self.height);
        for id in ids {
            if id == "details.close" {
                return Some(true);
            }
            let Some(details) = self.details.as_mut() else { continue };
            match id {
                // Enter keeps the search; esc drops it.
                "details.search-erase" | "details.search-keep" | "details.search-drop" => {
                    let mut query = details.query.clone();
                    match id {
                        "details.search-erase" => {
                            query.pop();
                        }
                        "details.search-keep" => {
                            self.searching = false;
                            return Some(false);
                        }
                        _ => {
                            self.searching = false;
                            query.clear();
                        }
                    }
                    details.search(&query, width);
                    details.show_match(width, height);
                }
                "details.up" | "details.down" | "details.page-up" | "details.page-down" | "details.top" | "details.end" => {
                    let page = height.saturating_sub(1).max(1) as i32;
                    let rows = match id {
                        "details.up" => -1,
                        "details.down" => 1,
                        "details.page-up" => -page,
                        "details.page-down" => page,
                        "details.top" => i32::MIN / 2,
                        _ => i32::MAX / 2,
                    };
                    details.scroll(rows, width, height);
                }
                "details.search" => {
                    self.searching = true;
                    details.search("", width);
                }
                "details.next-match" | "details.previous-match" => details.next_match(id == "details.next-match", width, height),
                "details.next-request" | "details.previous-request" => details.next_request(id == "details.next-request", width),
                "details.print" => app.print_details(details.printed(app.0.tty.size().columns - 1)),
                "details.edit" => app.edit_details(self.markdown.clone()),
                _ => continue,
            }
            return Some(false);
        }
        None
    }

    /// A character typed into the search while it's being typed.
    fn search_typed(&mut self, typed: char) {
        let (width, height) = (self.width, self.height);
        if let Some(details) = self.details.as_mut() {
            let query = format!("{}{typed}", details.query);
            details.search(&query, width);
            details.show_match(width, height);
        }
    }
}

impl Overlay<TuiApp> for DetailsPanel {
    /// The view's keys (keys.json can say others); while searching, what's typed goes into the search (enter keeps it,
    /// esc drops it).
    fn handle(&mut self, app: &TuiApp, event: InputEvent) -> Handled {
        match event {
            InputEvent::Key { .. } => {
                let ids = app.actions(self.scope(), &event);
                if self.act(app, ids) == Some(true) {
                    return Handled::Close;
                }
                Handled::Kept
            }
            // Typed fast, or by a test, a run of keys comes as one text: each is taken in turn.
            InputEvent::Text { ref text } => {
                for typed in text.chars() {
                    if self.searching {
                        self.search_typed(typed);
                        continue;
                    }
                    let ids = app.actions(Scope::Details, &InputEvent::text(&typed.to_string()));
                    if self.act(app, ids) == Some(true) {
                        return Handled::Close;
                    }
                }
                Handled::Kept
            }
            InputEvent::Paste { ref text } if self.searching => {
                for typed in sanitize_text(text, &[]).replace('\n', " ").chars() {
                    self.search_typed(typed);
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
    fn hint(&self, app: &TuiApp) -> String {
        let requests: (&[&str], &str) = (&["details.previous-request", "details.next-request"], "requests");
        if self.searching {
            app.keys_hint(&[(&[], "type to search"), (&["details.search-keep"], "keeps it"), (&["details.search-drop"], "drops it")])
        } else if self.details.as_ref().is_some_and(|details| !details.query.is_empty()) {
            app.keys_hint(&[
                (&["details.next-match", "details.previous-match"], "matches"),
                (&["details.search"], "searches again"),
                requests,
                (&["details.close"], "closes"),
            ])
        } else {
            app.keys_hint(&[
                (&["details.search"], "search"),
                requests,
                (&["details.print"], "scrollback"),
                (&["details.edit"], "editor"),
                (&["details.close"], "closes"),
            ])
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
