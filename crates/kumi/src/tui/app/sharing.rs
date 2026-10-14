//! /export and /share: this conversation as a file in the folder Kumi was started in, never over another, with what
//! `kumi report` takes out taken out.
use super::*;
use crate::export::{conversation_now, redactor_now, save_in, saved_words, Exported, COPY_MOST};
use kumi_runtime::core::export::Format;
use kumi_runtime::system;
use std::path::PathBuf;

impl TuiApp {
    /// /export: copy this conversation as Markdown, or save it as Markdown, a web page or a file for Kumi.
    pub(super) fn open_export(&self) {
        let item = |label: &str, value: &str, detail: &str| PickerItem { detail: Some(detail.into()), ..PickerItem::new(label, value) };
        let carried = format!("another Kumi carries it on: {} import", self.0.options.command);
        let here = self.0.options.folder.clone().or_else(|| std::env::current_dir().ok());
        let here = here.map(|folder| self.clean(&folder.display().to_string(), 120)).unwrap_or_default();
        let items = vec![
            item("Copy as Markdown", "copy", "to paste anywhere"),
            item("Save Markdown", "md", "a .md file"),
            item("Save web page", "html", "one page that opens anywhere"),
            item("Save for Kumi", "json", &carried),
        ];
        let title = if here.is_empty() {
            "Export this conversation".to_string()
        } else {
            format!("Export this conversation · files go in {here}")
        };
        self.pick(Picker::new(title, items), |app, item| async move {
            app.close_panel();
            match item.value.as_deref() {
                Some("copy") => app.copy_conversation().await,
                Some("html") => drop(app.save_conversation_as(Format::Html, false).await),
                Some("json") => drop(app.save_conversation_as(Format::Json, false).await),
                _ => drop(app.save_conversation_as(Format::Markdown, false).await),
            }
            Ok(())
        });
    }

    /// /export with words: copy, or save as md, html or json (as plain mode takes them); alone, the choices.
    pub(super) async fn export_as(&self, words: &str) {
        match words.to_ascii_lowercase().as_str() {
            "" => self.open_export(),
            "copy" => self.copy_conversation().await,
            "md" | "markdown" => drop(self.save_conversation_as(Format::Markdown, false).await),
            "html" | "page" | "web" => drop(self.save_conversation_as(Format::Html, false).await),
            "json" | "kumi" => drop(self.save_conversation_as(Format::Json, false).await),
            _ => self.notice(
                "Use /export alone for the choices, or /export copy, md, html or json (a file another Kumi imports).",
                NoticeTone::Info,
            ),
        }
    }

    /// /share: this conversation as a web page (the one saved last, while nothing in the conversation changed), then its
    /// path, to open, show in the file manager or copy, as many as the producer likes, until Done. Over SSH the page is on
    /// the computer Kumi runs on, where opening or showing it helps nobody: Kumi says where it is, to copy.
    pub(super) async fn share(&self) {
        let Some(path) = self.save_conversation_as(Format::Html, true).await else { return };
        let reveal = match system::platform() {
            "darwin" => "Show it in Finder",
            "win32" => "Show it in Explorer",
            _ => "Open its folder",
        };
        let remote = self.0.options.remote;
        let mut items = vec![];
        if !remote {
            items.extend([PickerItem::new("Open it", "open"), PickerItem::new(reveal, "reveal")]);
        }
        items.extend([PickerItem::new("Copy its path", "copy"), PickerItem::new("Done", "done")]);
        let shown = self.clean(&path.display().to_string(), 200);
        let title = if remote { format!("Saved on the computer Kumi runs on (over SSH): {shown}") } else { format!("Saved: {shown}") };
        self.pick(Picker::new(title, items), move |app, item| {
            let path = path.clone();
            async move {
                match item.value.as_deref() {
                    Some("open") => {
                        if let Some(open) = &app.0.options.open_browser {
                            open(&path.display().to_string());
                        }
                    }
                    Some("reveal") => {
                        if let Some(reveal) = &app.0.options.reveal {
                            reveal(path.as_path());
                        }
                    }
                    Some("copy") => {
                        app.copy_text(&path.display().to_string());
                        app.notice("Copied the page's path.", NoticeTone::Info);
                    }
                    _ => app.close_panel(),
                }
                Ok(())
            }
        });
    }

    /// Copy as Markdown: through the terminal, or, longer than a terminal takes, saved as a file, saying so.
    async fn copy_conversation(&self) {
        let Some(exported) = self.exported(Format::Markdown).await else { return };
        if exported.text.len() > COPY_MOST {
            match save_in(self.0.options.folder.as_deref(), &exported.text, &exported.name) {
                Ok(path) => self.notice(
                    &format!(
                        "This conversation is too long to copy through the terminal (over 64 KB), so Kumi saved it as {}.",
                        path.display()
                    ),
                    NoticeTone::Info,
                ),
                Err(why) => self.notice(&why, NoticeTone::Warn),
            }
            return;
        }
        self.copy_text(&exported.text);
        self.notice(
            "Copied this conversation as Markdown. If it didn't arrive, your terminal may not allow it: /export saves it as a file instead.",
            NoticeTone::Info,
        );
    }

    /// Saves this conversation in `format` and says where; None (after saying why) when it couldn't. With `again`, a web
    /// page saved last for the conversation as it is now is the one, rather than another just like it.
    async fn save_conversation_as(&self, format: Format, again: bool) -> Option<PathBuf> {
        let exported = self.exported(format).await?;
        if format == Format::Html {
            let saved = self.0.state.borrow().shared.clone().filter(|(key, path)| *key == exported.key && path.exists());
            if let Some((_, path)) = saved.filter(|_| again) {
                self.notice(&format!("This conversation's web page is saved already: {}.", path.display()), NoticeTone::Info);
                return Some(path);
            }
        }
        match save_in(self.0.options.folder.as_deref(), &exported.text, &exported.name) {
            Ok(path) => {
                if format == Format::Html {
                    self.0.state.borrow_mut().shared = Some((exported.key, path.clone()));
                }
                self.notice(&saved_words(format, &path, &self.0.options.command), NoticeTone::Info);
                Some(path)
            }
            Err(why) => {
                self.notice(&why, NoticeTone::Warn);
                None
            }
        }
    }

    /// This conversation in `format`, redacted, with its file's name and key; None (after saying so) before there's
    /// anything to export.
    async fn exported(&self, format: Format) -> Option<Exported> {
        let secrets = self.0.state.borrow().secrets.clone();
        let redact = redactor_now(self.0.options.redact.as_ref(), &secrets).await;
        match conversation_now(&*self.0.options.controller, format, &*redact).await {
            Ok(exported) => Some(exported),
            Err(why) => {
                self.notice(&why, NoticeTone::Info);
                None
            }
        }
    }
}
