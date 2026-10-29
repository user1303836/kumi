use super::super::{
    picker::{NoteTone, PickerOptions, PickerTabs},
    render::Cursor,
    width::graphemes,
};
use super::*;
use crate::{
    models::{ChatGptSignIn, LocalStatus, ProviderStatus},
    voice::VoiceChange,
};
use base64::Engine;
use kumi_agent::core::contracts::Picked;
use kumi_ai::failure::reset_words;
use kumi_ai::providers::{
    models::{ApiKeyCheck, FactsFrom, ModelInfo, ServiceTier},
    Effort, SignIn,
};
use kumi_common::time::since;
use kumi_types::errors::FailureKind;
use kumi_types::place::Place;
use std::collections::HashMap;

/// What the conversations list says of its keys.
fn conversations_hint() -> String {
    format!("enter goes back · tab: this {} or all · ctrl+f rename, pin, delete, copy id · esc closes", crate::daw::words().place)
}
/// After choosing a model: say when `/fast` puts it on its faster tier, since that costs more.
fn fast_note(tier: Option<&ServiceTier>) -> String {
    tier.map(|tier| {
        format!(" {} is on{}; /fast turns it off.", tier.name, tier.description.as_ref().map(|d| format!(": {d}")).unwrap_or_default())
    })
    .unwrap_or_default()
}
/// A word of a /recipe line: quoted when it has spaces, quotes or backslashes, so the line reads back the same.
fn recipe_word(text: &str) -> String {
    // A name like 808 or true goes in quotes too, so it stays a name rather than a number or a switch.
    let reads_as_value = serde_json::from_str::<Value>(text).is_ok_and(|v| v.is_number() || v.is_boolean());
    if !text.is_empty() && !reads_as_value && !text.chars().any(|c| c.is_whitespace() || c == '"' || c == '\\') {
        return text.into();
    }
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}
/// What the pinned object fills in for a blank named for it: `track` takes its track; `clip`, `device`,
/// `scene` and the like take the object itself.
fn pinned_value(pin: &PinnedNode, blank: &str) -> Option<String> {
    let kind = match pin.node {
        PinKind::Device => "device",
        PinKind::Chain => "chain",
        PinKind::Track => "track",
        PinKind::Clip => "clip",
        PinKind::Scene => "scene",
        PinKind::ClipSlot => "slot",
        PinKind::Selection => return None,
    };
    if blank.contains(kind) {
        Some(pin.r#ref.clone())
    } else if blank.contains("track") && !pin.track_ref.is_empty() {
        Some(pin.track_ref.clone())
    } else {
        None
    }
}
/// The /recipe line that runs `recipe` with no model call, each blank filled from what's pinned where it can
/// be, and how many characters of it follow the first blank left empty (where the cursor goes).
pub(super) fn recipe_line(recipe: &RecipeSummary, pinned: Option<&PinnedNode>) -> (String, usize) {
    let mut line = format!("/recipe {}", recipe_word(&recipe.name));
    let mut empty = None;
    for blank in &recipe.params {
        line.push_str(&format!(" {}=", blank.name));
        match pinned.and_then(|pin| pinned_value(pin, &blank.name)) {
            Some(value) => line.push_str(&recipe_word(&value)),
            None => {
                empty.get_or_insert(graphemes(&line).len());
            }
        }
    }
    let after = empty.map_or(0, |at| graphemes(&line).len() - at);
    (line, after)
}
/// `/recipe <name> blank=value …` read back: the recipe's name and a value for each blank it names.
pub(super) fn recipe_command(line: &str) -> Result<(String, JsonObject), String> {
    let how = || "Run a recipe with: /recipe <name> blank=value … (a value with spaces goes in quotes)".to_string();
    // Each word, and whether any of it was in quotes.
    let mut words: Vec<(String, bool)> = Vec::new();
    let mut word: Option<(String, bool)> = None;
    let mut quoted = false;
    let mut chars = line.strip_prefix("/recipe").unwrap_or(line).chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    word.get_or_insert_with(Default::default).0.push(next);
                }
            }
            '"' => {
                quoted = !quoted;
                word.get_or_insert_with(Default::default).1 = true;
            }
            c if c.is_whitespace() && !quoted => words.extend(word.take()),
            c => word.get_or_insert_with(Default::default).0.push(c),
        }
    }
    if quoted {
        return Err(how());
    }
    words.extend(word);
    let mut words = words.into_iter();
    let name = words.next().ok_or_else(how)?.0;
    let mut with = JsonObject::new();
    for (word, quoted) in words {
        let (blank, value) = word.split_once('=').ok_or_else(how)?;
        // bpm=124 is a number and on=true a switch, as the model would pass them; "124" in quotes stays words.
        let value = match serde_json::from_str::<Value>(value) {
            Ok(value @ (Value::Number(_) | Value::Bool(_))) if !quoted => value,
            _ => Value::String(value.into()),
        };
        with.insert(blank.into(), value);
    }
    Ok((name, with))
}
/// Why a /recipe line can't run yet: a blank it doesn't fill (with what it's for), or one the recipe doesn't have.
pub(super) fn recipe_blanks_problem(recipe: &RecipeSummary, with: &JsonObject) -> Option<String> {
    if let Some(unknown) = with.keys().find(|k| !recipe.params.iter().any(|p| &p.name == *k)) {
        let blanks = recipe.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
        return Some(if blanks.is_empty() {
            format!("“{}” has no blanks to fill: /recipe {} runs it.", recipe.name, recipe_word(&recipe.name))
        } else {
            format!("“{}” has no blank called {unknown}; its blanks are {blanks}.", recipe.name)
        });
    }
    let empty: Vec<_> = recipe
        .params
        .iter()
        // A number or a switch fills its blank; words fill it unless they're blank.
        .filter(|p| match with.get(&p.name) {
            None | Some(Value::Null) => true,
            Some(Value::String(words)) => string::trim(words).is_empty(),
            Some(_) => false,
        })
        .map(|p| if p.about.is_empty() { p.name.clone() } else { format!("{} ({})", p.name, p.about) })
        .collect();
    (!empty.is_empty()).then(|| format!("“{}” needs {}.", recipe.name, empty.join(", ")))
}
/// How long Kumi's window has to be behind another before Kumi answers its own question for the producer.
const UNATTENDED_MS: f64 = 120_000.;
pub(super) fn item(label: impl Into<String>, value: impl Into<String>, detail: impl Into<String>) -> PickerItem {
    let detail = detail.into();
    PickerItem { detail: (!detail.is_empty()).then_some(detail), ..PickerItem::new(label, value) }
}
pub(super) fn inert(label: impl Into<String>) -> PickerItem {
    PickerItem { label: label.into(), inert: true, ..Default::default() }
}
pub(super) fn noted(mut item: PickerItem, note: impl Into<String>, tone: NoteTone) -> PickerItem {
    item.note = Some(note.into());
    item.note_tone = Some(tone);
    item
}
impl TuiApp {
    pub(super) fn recall_older(&self) {
        let entries = self.0.options.history.as_ref().map(|h| h.borrow().entries().to_vec()).unwrap_or_default();
        let mut state = self.0.state.borrow_mut();
        let index = state.recall.as_ref().map(|r| r.0).unwrap_or(entries.len());
        if index == 0 {
            return;
        }
        let index = index - 1;
        // The draft is set aside the first time, with what undo can take back in it; a message shown since just goes.
        let shown = state.editor.show(&entries[index]);
        let draft = state.recall.take().map(|(_, draft)| draft).unwrap_or(shown);
        state.recall = Some((index, draft));
        state.menu_dismissed = true;
    }
    pub(super) fn recall_newer(&self) {
        let entries = self.0.options.history.as_ref().map(|h| h.borrow().entries().to_vec()).unwrap_or_default();
        let mut state = self.0.state.borrow_mut();
        let Some((index, draft)) = state.recall.take() else {
            return;
        };
        let index = index + 1;
        if index >= entries.len() {
            state.editor.restore(draft);
            return;
        }
        state.editor.show(&entries[index]);
        state.recall = Some((index, draft));
        state.menu_dismissed = true;
    }
    pub(super) async fn open_memory(&self) -> Result<(), RuntimeError> {
        let s = self.session();
        let (memory, techniques, recipes, lessons, taste) = futures::try_join!(
            s.op(op::Memory {}),
            s.op(op::Techniques {}),
            s.op(op::Recipes {}),
            s.op(op::Lessons {}),
            s.op(op::Taste {})
        )?;
        let Some(memory) = memory else {
            return Ok(());
        };
        let now = now_ms_f64();
        let mut items = vec![PickerItem::heading("About you")];
        let notes = |notes: &[MemoryNote]| {
            notes
                .iter()
                .rev()
                .map(|n| {
                    let age = since(n.at as f64, now);
                    noted(
                        PickerItem::new(&n.text, format!("note:{}", n.id)),
                        if n.pinned { format!("pinned · {age}") } else { age },
                        NoteTone::Faint,
                    )
                })
                .collect::<Vec<_>>()
        };
        let kept: Vec<MemoryNote> = memory.memory.producer.iter().chain(&memory.memory.set).cloned().collect();
        if memory.memory.producer.is_empty() {
            items.push(inert("Nothing yet"));
        } else {
            items.extend(notes(&memory.memory.producer));
        }
        let place = crate::daw::words().place;
        items.push(PickerItem::heading(format!("About {}", memory.set_name.clone().unwrap_or_else(|| format!("this {place}")))));
        if !memory.saved {
            items.push(inert(&format!("Kept once the {place} is saved")));
        } else if memory.memory.set.is_empty() {
            items.push(inert("Nothing yet"));
        } else {
            items.extend(notes(&memory.memory.set));
        }
        if self.can().techniques {
            items.push(PickerItem::heading("Techniques"));
            if techniques.is_empty() {
                items.push(inert("None yet: Kumi asks before it keeps one"));
            } else {
                items.extend(techniques.iter().map(|t| {
                    let i = item(
                        self.clean(&t.name.replace('\n', " "), 60),
                        format!("technique:{}", t.id),
                        self.clean(&t.fits.replace('\n', " "), 160),
                    );
                    if let Some(source) = t.source.as_ref().filter(|s| !s.is_empty()) {
                        noted(i, self.clean(&source.replace('\n', " "), 40), NoteTone::Faint)
                    } else {
                        i
                    }
                }));
            }
        }
        if self.can().recipes {
            items.push(PickerItem::heading("Recipes"));
            if recipes.is_empty() {
                items.push(inert("None yet"));
            } else {
                items.extend(
                    recipes.iter().map(|r| {
                        noted(item(&r.name, format!("recipe:{}", r.name), &r.about), format!("{} steps", r.steps), NoteTone::Faint)
                    }),
                );
            }
        }
        if self.can().lessons {
            items.push(PickerItem::heading("What Kumi learned matching sounds"));
            if lessons.is_empty() {
                items.push(inert("None yet: what won when Kumi matched a sound to a reference"));
            } else {
                items.extend(lessons.iter().map(|l| {
                    noted(
                        PickerItem::new(self.clean(&l.line.replace('\n', " "), 160), format!("lesson:{}", l.id)),
                        since(l.at, now),
                        NoteTone::Faint,
                    )
                }));
            }
        }
        if self.can().taste {
            let places = crate::daw::words().places;
            items.push(PickerItem::heading(format!("From your {places}")));
            if taste.is_empty() {
                items.push(inert(
                    if self
                        .0
                        .state
                        .borrow()
                        .library
                        .as_ref()
                        .is_some_and(|l| matches!(l.state, LibraryState::Learning | LibraryState::Paused))
                    {
                        format!("Learning your {places}…")
                    } else {
                        format!("Nothing yet: how you work, from your own {} {places}", crate::daw::words().app)
                    }
                    .as_str(),
                ));
            } else {
                items.extend(taste.iter().map(|l| PickerItem::new(self.clean(&l.line.replace('\n', " "), 160), format!("taste:{}", l.id))));
            }
        }
        self.pick(
            Picker::with_options(
                "What Kumi remembers · notes, techniques and recipes",
                items,
                PickerOptions { filterable: true, ..Default::default() },
            ),
            move |app, selected| {
                let recipes = recipes.clone();
                let kept = kept.clone();
                async move {
                    let (kind, id) = selected.value.as_deref().unwrap_or("").split_once(':').unwrap_or(("", ""));
                    if kind == "recipe" {
                        if let Some(recipe) = recipes.iter().find(|r| r.name == id) {
                            app.recipe_actions(recipe.clone());
                        }
                        return Ok(());
                    }
                    if kind == "note" && app.can().change_note {
                        if let Some(note) = kept.iter().find(|n| n.id == id) {
                            app.note_actions(note.clone());
                        }
                        return Ok(());
                    }
                    let title = match kind {
                        "technique" => "Forget this technique?",
                        "lesson" => "Forget this lesson?",
                        "taste" => "Forget this, from your Sets?",
                        _ => "Forget this note?",
                    };
                    let kind = kind.to_string();
                    let id = id.to_string();
                    let label = selected.label.clone();
                    app.pick(
                        Picker::new(title, vec![item("Forget it", "yes", &selected.label), PickerItem::new("Keep it", "no")]),
                        move |app, answer| {
                            let (kind, id, label) = (kind.clone(), id.clone(), label.clone());
                            async move {
                                app.close_panel();
                                if answer.value.as_deref() != Some("yes") {
                                    return Ok(());
                                }
                                let s = app.session();
                                let gone = match kind.as_str() {
                                    "technique" => s.op(op::ForgetTechnique { id: id.clone() }).await?,
                                    "lesson" => s.op(op::ForgetLesson { id: id.clone() }).await?,
                                    "taste" => s.op(op::ForgetTaste { id: id.clone() }).await?,
                                    _ => s.op(op::Forget { id: id.clone() }).await?.is_some(),
                                };
                                if !gone {
                                    app.notice(
                                        &format!(
                                            "That {} was already gone.",
                                            match kind.as_str() {
                                                "technique" | "lesson" => &kind,
                                                "taste" => "line",
                                                _ => "note",
                                            }
                                        ),
                                        NoticeTone::Info,
                                    );
                                } else if kind == "taste" {
                                    app.notice(&format!("Forgot, from your Sets: {label}"), NoticeTone::Info);
                                }
                                Ok(())
                            }
                        },
                    );
                    Ok(())
                }
            },
        );
        Ok(())
    }
    /// The kept conversations to go back to, this Set's and (under Tab) every Set's, which come first while no Set is
    /// known. Enter goes back to one; ctrl+f renames, pins or deletes it, or copies its id. `filter`: typed in already.
    pub(super) async fn open_conversations(&self, filter: Option<String>) -> Result<(), RuntimeError> {
        let here = self.session().op(op::Conversations {}).await?;
        let all = self.session().op(op::AllConversations {}).await?;
        let now = now_ms_f64();
        // A row's title is cut short enough to leave its Set (in All Sets) and its id room beside it, as far as they go.
        let row = |row: &ConversationSummary, every: bool| {
            let title = self.clean(&row.title(), 60);
            let parts = graphemes(&title);
            let title = if parts.len() > 30 { format!("{}…", string::trim_end(&parts[..29].concat())) } else { title };
            let set = match row.set.as_deref() {
                Some(name) => self.clean(name, 20),
                None if matches!(Place::parse(&row.place), Some(Place::Unsaved(_))) => {
                    format!("A {} not saved yet", crate::daw::words().place)
                }
                None if row.place == kumi_types::place::IMPORTED => "Imported".into(),
                None => format!("A saved {}", crate::daw::words().place),
            };
            let age = if row.current { "this one".into() } else { since(row.saved_at as f64, now) };
            let mut item = noted(
                item(
                    if title.is_empty() { "(nothing asked yet)".into() } else { title },
                    &row.id,
                    if every { format!("{set} · {}", row.id) } else { row.id.clone() },
                ),
                format!(
                    "{}{age} · {} {}",
                    if row.pinned { "pinned · " } else { "" },
                    row.turns,
                    if row.turns == 1 { "request" } else { "requests" }
                ),
                NoteTone::Faint,
            );
            // Found by its whole name too, past where the row cuts it.
            let name = row.name.as_deref().map(|name| self.clean(name, 60)).unwrap_or_default();
            item.words = Some(format!("{name} {} {set}", self.clean(&row.first.replace('\n', " "), 200)));
            item
        };
        let list = |rows: &[ConversationSummary], every: bool| {
            if rows.is_empty() {
                vec![inert("None kept yet: a conversation is kept once you've asked something")]
            } else {
                rows.iter().map(|r| row(r, every)).collect()
            }
        };
        let known = self.0.state.borrow().set_name.is_some() || !here.is_empty();
        let tabs = PickerTabs {
            names: vec![
                format!("This {}", crate::daw::capitalized(crate::daw::words().place)),
                format!("All {}", crate::daw::capitalized(crate::daw::words().places)),
            ],
            lists: vec![list(&here, false), list(&all, true)],
            at: if known && filter.as_deref().is_none_or(str::is_empty) { 0 } else { 1 },
        };
        let options = PickerOptions { filterable: true, alternate: true, hint: Some(conversations_hint()), ..Default::default() };
        let mut picker = Picker::with_tabs("Conversations", tabs, options);
        picker.filter = filter.map(|f| string::head(&f, 40)).unwrap_or_default();
        let kept: Vec<ConversationSummary> = all.iter().chain(here.iter()).cloned().collect();
        self.pick(picker, move |app, item| {
            let value = item.value.clone().unwrap_or_default();
            let (more, id) = match value.strip_prefix("fallback:") {
                Some(id) => (true, id.to_string()),
                None => (false, value),
            };
            let row = kept.iter().find(|r| r.id == id).cloned();
            async move {
                let Some(row) = row else { return Ok(()) };
                if more {
                    return app.conversation_actions(row).await;
                }
                app.close_panel();
                app.go_back_to(&row).await;
                Ok(())
            }
        });
        Ok(())
    }
    /// Go back to a kept conversation; one of another Set asks first.
    pub(super) async fn go_back_to(&self, row: &ConversationSummary) {
        if row.current {
            return;
        }
        if self.busy() {
            self.notice("Kumi is still working. Press esc to stop it first.", NoticeTone::Info);
            return;
        }
        self.0.state.borrow_mut().activity = "going back to it".into();
        match self.session().op(op::ResumeConversation { id: row.id.clone() }).await {
            Ok(false) => self.notice("That conversation isn't kept any more.", NoticeTone::Info),
            Err(error) => self.error(&error),
            _ => {}
        }
    }
    /// What else to do with a kept conversation: go back to it, rename, pin or delete it, or copy its id, under a look at
    /// how it ended.
    async fn conversation_actions(&self, row: ConversationSummary) -> Result<(), RuntimeError> {
        let lines = self.session().op(op::ConversationLines { id: row.id.clone() }).await.unwrap_or_default();
        let mut items = vec![];
        let last = |role: TranscriptRole| lines.iter().rev().find(|l| l.role == role && !l.text.is_empty());
        if let Some(asked) = last(TranscriptRole::User) {
            items.push(inert(format!("Last asked: {}", self.clean(&asked.text.replace('\n', " "), 100))));
        }
        if let Some(said) = last(TranscriptRole::Assistant) {
            items.push(inert(format!("Kumi: {}", self.clean(&said.text.replace('\n', " "), 100))));
        }
        if !row.current {
            items.push(item("Go back to it", "back", "Kumi's next answer carries on from it"));
        }
        items.push(item("Rename it", "rename", "In the box below; enter keeps the name"));
        items.push(if row.pinned {
            item("Unpin it", "pin", "Kept as long as other conversations again")
        } else {
            item("Pin it", "pin", "Kept however old it gets")
        });
        items.push(item("Copy its id", "copy", format!("{} -r {} brings it back", *KUMI_START, row.id)));
        if !row.current {
            items.push(item("Delete it", "delete", "For good"));
        }
        items.push(PickerItem::new("Leave it as it is", "keep"));
        let title = format!("“{}” · {}", self.clean(&row.title(), 50), row.id);
        self.pick(Picker::new(title, items), move |app, answer| {
            let row = row.clone();
            async move {
                app.close_panel();
                match answer.value.as_deref() {
                    Some("back") => app.go_back_to(&row).await,
                    Some("rename") => {
                        app.0.state.borrow_mut().editor.set(&format!("/rename {} {}", row.id, row.name.as_deref().unwrap_or(&row.title())));
                        app.0.scheduler.request();
                    }
                    Some("pin") => match app.session().op(op::PinConversation { id: row.id.clone(), pinned: !row.pinned }).await? {
                        true if !row.pinned => app.notice("Pinned: Kumi keeps it however old it gets.", NoticeTone::Info),
                        true => app.notice("Unpinned.", NoticeTone::Info),
                        false => app.notice("That conversation isn't kept any more.", NoticeTone::Info),
                    },
                    Some("copy") => {
                        app.copy_text(&row.id);
                        app.notice(&format!("Copied its id, {}.", row.id), NoticeTone::Info);
                    }
                    Some("delete") => app.confirm_delete(row),
                    _ => {}
                }
                Ok(())
            }
        });
        Ok(())
    }
    fn confirm_delete(&self, row: ConversationSummary) {
        let title = format!("Delete “{}”?", self.clean(&row.title(), 50));
        self.pick(
            Picker::new(title, vec![item("Delete it", "yes", "It goes for good"), PickerItem::new("Keep it", "no")]),
            move |app, answer| {
                let id = row.id.clone();
                async move {
                    app.close_panel();
                    if answer.value.as_deref() != Some("yes") {
                        return Ok(());
                    }
                    match app.session().op(op::DeleteConversation { id: id.clone() }).await {
                        Ok(true) => app.notice("Deleted.", NoticeTone::Info),
                        Ok(false) => app.notice("That conversation was already gone.", NoticeTone::Info),
                        Err(error) => app.error(&error),
                    }
                    Ok(())
                }
            },
        );
    }
    /// The conversation the producer chose belongs to another Set: open that Set, carry it on here as a copy, or leave it.
    /// None: the question was answered or is no longer open.
    pub(super) fn open_set_ask(&self, ask: Option<SetAsk>) {
        let Some(ask) = ask else {
            let open = self.0.state.borrow_mut().set_ask.take();
            if open.is_some_and(|open| self.same_picker(&open)) {
                self.close_panel();
            }
            return;
        };
        let (daw, noun) = (self.clean(&ask.daw, 20), self.clean(&ask.noun, 20));
        let theirs = ask.set.as_deref().map(|set| format!("“{}”", self.clean(set, 40))).unwrap_or_else(|| format!("its {noun}"));
        let open = ask.open.as_deref().map(|set| format!("“{}”", self.clean(set, 40))).unwrap_or_else(|| format!("a {noun} not saved yet"));
        let title = if ask.title.is_empty() { "This conversation".to_string() } else { format!("“{}”", self.clean(&ask.title, 40)) };
        // A DAW Kumi can't open places in: the producer opens it, and nothing recommends what Kumi can't do.
        let open_it = if ask.by_hand {
            item(format!("Open {theirs} in {daw} yourself"), "open", "This conversation carries on once it's open")
        } else {
            noted(
                item(format!("Open {theirs} in {daw}"), "open", format!("{daw} may ask whether to save {open} first")),
                "recommended",
                NoteTone::Accent,
            )
        };
        let items = vec![
            open_it,
            item("Carry it on here, as a copy", "copy", format!("The original stays with {theirs}")),
            item("Cancel", "cancel", format!("Stay with {open}'s own conversation")),
        ];
        let picker = self.pick(Picker::new(format!("{title} is about {theirs}; {daw} has {open} open"), items), |app, answer| async move {
            app.0.state.borrow_mut().set_ask = None;
            app.close_panel();
            let choice = match answer.value.as_deref() {
                Some("open") => SetChoice::Open,
                Some("copy") => SetChoice::Copy,
                _ => SetChoice::Cancel,
            };
            app.0.state.borrow_mut().activity =
                if choice == SetChoice::Copy { "carrying it on here".into() } else { format!("opening the {}", crate::daw::words().place) };
            app.session().op(op::AnswerSet { choice }).await
        });
        self.0.state.borrow_mut().set_ask = Some(picker);
    }
    pub(super) async fn open_recipes(&self) -> Result<(), RuntimeError> {
        let recipes = self.session().op(op::Recipes {}).await?;
        let now = now_ms_f64();
        let items = if recipes.is_empty() {
            vec![inert(&format!("None yet: ask Kumi to save a way of working, or say “watch me” and do it in {}", crate::daw::words().app))]
        } else {
            recipes
                .iter()
                .map(|r| {
                    noted(
                        item(&r.name, &r.name, &r.about),
                        if r.used != 0. {
                            format!("used {}", since(r.last_used.unwrap_or(r.created), now))
                        } else {
                            format!("{} steps", r.steps)
                        },
                        NoteTone::Faint,
                    )
                })
                .collect()
        };
        self.pick(
            Picker::with_options(
                "Your recipes · ways of working Kumi replays without planning again",
                items,
                PickerOptions { filterable: true, ..Default::default() },
            ),
            move |app, item| {
                let recipe = recipes.iter().find(|r| Some(&r.name) == item.value.as_ref()).cloned();
                async move {
                    if let Some(recipe) = recipe {
                        app.recipe_actions(recipe);
                    }
                    Ok(())
                }
            },
        );
        Ok(())
    }
    /// A note's words to change (in the box, as /note), its pin, or forgetting it.
    fn note_actions(&self, note: MemoryNote) {
        let title = format!("“{}”", self.clean(&note.text, 60));
        self.pick(
            Picker::new(
                title,
                vec![
                    item("Change the words", "change", "In the box below; press enter to keep them"),
                    if note.pinned {
                        item("Unpin it", "pin", "A full memory can make room by dropping it again")
                    } else {
                        item("Pin it", "pin", "A full memory never drops it to make room")
                    },
                    PickerItem::new("Forget it", "forget"),
                    PickerItem::new("Keep it", "keep"),
                ],
            ),
            move |app, answer| {
                let note = note.clone();
                async move {
                    app.close_panel();
                    let s = app.session();
                    match answer.value.as_deref() {
                        Some("change") => {
                            app.0.state.borrow_mut().editor.set(&format!("/note {} {}", note.id, note.text));
                            app.0.scheduler.request();
                        }
                        Some("pin") => {
                            match s.op(op::ChangeNote { id: note.id.clone(), change: NoteChange::Pinned(!note.pinned) }).await? {
                                Some(changed) if changed.pinned => {
                                    app.notice("Pinned: Kumi keeps this note even when its memory is full.", NoticeTone::Info)
                                }
                                Some(_) => app.notice("Unpinned.", NoticeTone::Info),
                                None => app.notice("That note was already gone.", NoticeTone::Info),
                            }
                        }
                        Some("forget") => {
                            if s.op(op::Forget { id: note.id.clone() }).await?.is_none() {
                                app.notice("That note was already gone.", NoticeTone::Info);
                            }
                        }
                        _ => {}
                    }
                    Ok(())
                }
            },
        );
    }
    fn recipe_actions(&self, recipe: RecipeSummary) {
        let blanks =
            recipe.params.iter().map(|p| if p.about.is_empty() { p.name.as_str() } else { &p.about }).collect::<Vec<_>>().join(", ");
        self.pick(
            Picker::new(
                format!("“{}” · {} steps", recipe.name, recipe.steps),
                vec![
                    item(
                        if recipe.params.is_empty() { "Run it now" } else { "Run it on…" },
                        "run",
                        if recipe.params.is_empty() { recipe.about.clone() } else { format!("Kumi needs: {blanks}") },
                    ),
                    PickerItem::new("Forget it", "forget"),
                    PickerItem::new("Keep it", "keep"),
                ],
            ),
            move |app, answer| {
                let recipe = recipe.clone();
                async move {
                    app.close_panel();
                    if answer.value.as_deref() == Some("forget") {
                        if !app.session().op(op::ForgetRecipe { name: recipe.name.clone() }).await? {
                            app.notice("That recipe was already gone.", NoticeTone::Info);
                        }
                        return Ok(());
                    }
                    if answer.value.as_deref() != Some("run") {
                        return Ok(());
                    }
                    if !recipe.params.is_empty() {
                        // Its blanks go on a /recipe line, filled from what's pinned where a blank's name says what
                        // it is; enter runs it with no model call. The cursor waits at the first one left empty.
                        // (as the DAW's integration carries it to now: a Renoise pin's places move with its tracks)
                        let pinned = app.0.state.borrow().pinned.as_ref().map(|p| p.pin.clone());
                        let pinned = match pinned {
                            Some(pin) => app.session().op(op::PinForRecipe { pin }).await?,
                            None => None,
                        };
                        let (line, after) = recipe_line(&recipe, pinned.as_ref());
                        let mut state = app.0.state.borrow_mut();
                        state.editor.set(&line);
                        for _ in 0..after {
                            state.editor.left();
                        }
                        drop(state);
                        app.0.scheduler.request();
                        return Ok(());
                    }
                    app.run_recipe_now(&recipe, JsonObject::new()).await;
                    Ok(())
                }
            },
        );
    }
    /// Run a recipe with its blanks filled, straight away: no model call.
    pub(super) async fn run_recipe_now(&self, recipe: &RecipeSummary, with: JsonObject) {
        if self.busy() {
            self.notice("Kumi is still working. Press esc to stop it first.", NoticeTone::Info);
            return;
        }
        self.0.state.borrow_mut().activity = format!("running “{}”", recipe.name);
        if self.can().run_recipe {
            let outcome = self.session().op(op::RunRecipe { name: recipe.name.clone(), with }).await.unwrap_or_else(|error| {
                RecipeOutcome { text: safe_error_message(Some(&error.message()), &self.0.state.borrow().secrets), is_error: true }
            });
            self.notice(
                &if outcome.is_error { format!("The recipe stopped: {}", outcome.text) } else { outcome.text },
                if outcome.is_error { NoticeTone::Warn } else { NoticeTone::Info },
            );
        }
    }
    pub(super) fn sign_in(&self, provider: ProviderId, then: Option<Action>) {
        if self.0.options.models.is_none() {
            return;
        }
        if provider.row().sign_in == SignIn::ApiKey {
            self.open_panel(Panel::Key { provider, secret: String::new(), checking: false, status: None, then });
            return;
        }
        // Over SSH the browser isn't on this computer: a code, entered in a browser anywhere.
        self.sign_in_chatgpt(self.0.options.remote, then);
    }
    /// ChatGPT's sign-in: in this computer's browser, or `by_code`, with a code entered in a browser anywhere.
    pub(super) fn sign_in_chatgpt(&self, by_code: bool, then: Option<Action>) {
        let Some(models) = self.0.options.models.clone() else {
            return;
        };
        let abort = Signal::new();
        let panel = self.open_panel(Panel::ChatGpt { url: None, code: None, by_code, abort: abort.clone(), then });
        let weak = Rc::downgrade(&self.0);
        let shown = panel.clone();
        let how = if by_code {
            let on_code = Rc::new(move |prompt: kumi_ai::auth::openai_codex::DevicePrompt| {
                if let Panel::ChatGpt { code, .. } = &mut *shown.borrow_mut() {
                    *code = Some(prompt);
                }
                if let Some(a) = weak.upgrade() {
                    a.scheduler.request();
                }
            });
            ChatGptSignIn::Device { signal: abort.clone(), on_code }
        } else {
            let on_url = Rc::new(move |url: String| {
                if let Panel::ChatGpt { url: at, .. } = &mut *shown.borrow_mut() {
                    *at = Some(url.clone());
                }
                if let Some(a) = weak.upgrade() {
                    if let Some(open) = &a.options.open_browser {
                        open(&url);
                    }
                    a.scheduler.request();
                }
            });
            ChatGptSignIn::Browser { signal: abort.clone(), on_url }
        };
        self.task(move |app| async move {
            let result = models.sign_in_chatgpt(how).await;
            match result {
                Ok(()) => {
                    if app.0.state.borrow_mut().overlays.remove(&panel) {
                        app.notice("Signed in to ChatGPT.", NoticeTone::Info);
                        let then = if let Panel::ChatGpt { then, .. } = &*panel.borrow() { then.clone() } else { None };
                        if let Some(then) = then {
                            if let Err(error) = then().await {
                                if !abort.is_cancelled() && !app.0.state.borrow().closing {
                                    app.notice(
                                        &format!(
                                            "The ChatGPT sign-in didn't finish: {}",
                                            safe_error_message(Some(&error.message()), &app.0.state.borrow().secrets)
                                        ),
                                        NoticeTone::Warn,
                                    );
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    app.0.state.borrow_mut().overlays.remove(&panel);
                    if !abort.is_cancelled() && !app.0.state.borrow().closing {
                        let message = format!(
                            "The ChatGPT sign-in didn't finish: {}",
                            safe_error_message(Some(&error.message()), &app.0.state.borrow().secrets)
                        );
                        app.setup_failed(&message);
                        app.notice(&message, NoticeTone::Warn);
                    }
                }
            }
            app.0.scheduler.request();
            Ok(())
        });
    }
    async fn submit_key(&self, panel: Shared<TuiApp>) {
        let started = as_panel(&panel, |p| {
            let Panel::Key { provider, secret, checking, status, then } = p else {
                return None;
            };
            *checking = true;
            *status = Some((format!("Checking the key with {}…", provider.row().name), NoticeTone::Info));
            Some((*provider, secret.clone(), then.clone()))
        });
        let Some((provider, key, then)) = started.flatten() else {
            return;
        };
        self.0.scheduler.request();
        let name = provider.row().name;
        let verdict = match self.0.options.models.as_ref().unwrap().save_key(provider, &key, None).await {
            Ok(v) => v,
            Err(error) => {
                let mut secrets = self.0.state.borrow().secrets.clone();
                secrets.push(key);
                as_panel(&panel, |p| {
                    if let Panel::Key { checking, status, .. } = p {
                        *checking = false;
                        *status = Some((safe_error_message(Some(&error.message()), &secrets), NoticeTone::Warn));
                    }
                });
                self.0.scheduler.request();
                return;
            }
        };
        if verdict == ApiKeyCheck::Refused {
            as_panel(&panel, |p| {
                if let Panel::Key { checking, secret, status, .. } = p {
                    *checking = false;
                    secret.clear();
                    *status = Some((format!("{name} didn't accept that key. Paste it again, or esc to leave it."), NoticeTone::Warn));
                }
            });
            self.0.scheduler.request();
            return;
        }
        {
            let mut state = self.0.state.borrow_mut();
            state.secrets.push(key.clone());
            state.stream.add_secret(key);
        }
        as_panel(&panel, |p| {
            if let Panel::Key { secret, .. } = p {
                secret.clear();
            }
        });
        self.0.state.borrow_mut().overlays.remove(&panel);
        self.notice(
            &if verdict == ApiKeyCheck::Ok {
                format!("Signed in to {name}.")
            } else {
                format!("Kept your {name} key; {name} didn't answer just now, so it isn't checked yet.")
            },
            NoticeTone::Info,
        );
        if let Some(then) = then {
            if let Err(error) = then().await {
                self.panel_failed(&error);
            }
        }
        self.0.scheduler.request();
    }
    pub(super) fn offer_fix(&self, kind: FailureKind, provider: Option<&str>) {
        let Some(models) = &self.0.options.models else {
            return;
        };
        if !self.0.state.borrow().overlays.is_empty() {
            return;
        }
        if kind == FailureKind::Config && models.current().model.is_none() {
            self.task(|app| async move {
                if let Err(error) = app.open_models().await {
                    app.panel_failed(&error);
                }
                Ok(())
            });
            return;
        }
        if kind == FailureKind::Network && provider.is_some_and(|p| models.cloud_provider(p).is_none()) {
            let name = models.provider_name(provider.unwrap());
            self.pick(
                Picker::new(
                    "Send your message again?",
                    vec![
                        item("Send it again", "again", format!("once {name} is running")),
                        PickerItem::new("Choose another model", "model"),
                        PickerItem::new("Not now", "later"),
                    ],
                ),
                |app, item| async move {
                    app.close_panel();
                    match item.value.as_deref() {
                        Some("again") => app.resend(),
                        Some("model") => app.open_models().await?,
                        _ => {}
                    }
                    Ok(())
                },
            );
            return;
        }
        // A plan's limit that resets later than Kumi waits, credit used up, or a provider still busy or down past the
        // retries.
        let limited = matches!(kind, FailureKind::UsageLimit | FailureKind::Billing);
        if limited || matches!(kind, FailureKind::RateLimit | FailureKind::Provider) {
            let name = provider.map(|p| models.provider_name(p)).unwrap_or_else(|| "The provider".into());
            let mut items = Vec::new();
            if limited {
                items.push(item("Choose a fallback model", "fallback", "it carries on whenever this plan's limit is reached"));
            } else {
                items.push(item("Send it again", "again", format!("once {name} answers again")));
            }
            items.push(PickerItem::new("Choose another model", "model"));
            items.push(PickerItem::new("Sign in to another provider", "login"));
            items.push(PickerItem::new("Not now", "later"));
            let title = if limited { format!("Carry on without {name}?") } else { format!("{name} isn't answering. Carry on?") };
            self.pick(Picker::new(title, items), |app, item| async move {
                app.close_panel();
                match item.value.as_deref() {
                    Some("fallback") => app.open_fallbacks().await?,
                    Some("again") => app.resend(),
                    Some("model") => app.open_models().await?,
                    Some("login") => app.open_login().await?,
                    _ => {}
                }
                Ok(())
            });
            return;
        }
        if matches!(kind, FailureKind::Model | FailureKind::Config) {
            self.pick(
                Picker::new("Choose another model?", vec![PickerItem::new("Choose a model", "model"), PickerItem::new("Not now", "later")]),
                |app, item| async move {
                    app.close_panel();
                    if item.value.as_deref() == Some("model") {
                        app.open_models().await?;
                    }
                    Ok(())
                },
            );
            return;
        }
        let Some(provider) = provider.and_then(|p| models.cloud_provider(p)).filter(|_| kind == FailureKind::Auth) else {
            return;
        };
        let info = provider.row();
        self.pick(
            Picker::new(
                format!("Sign in to {}?", info.name),
                vec![
                    item(
                        "Sign in now",
                        "signin",
                        if info.sign_in == SignIn::Chatgpt { "with your ChatGPT plan, in the browser" } else { "with an API key" },
                    ),
                    PickerItem::new("Choose another model", "model"),
                    PickerItem::new("Not now", "later"),
                ],
            ),
            move |app, item| async move {
                app.close_panel();
                match item.value.as_deref() {
                    Some("signin") => app.sign_in(
                        provider,
                        Some(app.action(|app| async move {
                            app.resend();
                            Ok(())
                        })),
                    ),
                    Some("model") => app.open_models().await?,
                    _ => {}
                }
                Ok(())
            },
        );
    }
    fn resend(&self) {
        let Some(raw) = self.0.state.borrow().last_sent.clone().filter(|s| !s.is_empty()) else {
            return;
        };
        if self.busy() {
            return;
        }
        self.notice("Sending your message again.", NoticeTone::Info);
        self.task(move |app| async move {
            app.send(&raw).await;
            Ok(())
        });
    }
    /// Says which model Kumi chose by itself: the provider's own first choice, or a model on this computer.
    pub(super) fn told_default(&self, chosen: crate::models::DefaultModel) {
        let Some(models) = &self.0.options.models else { return };
        let model = chosen.model;
        let name = models.provider_name(&model.provider);
        self.notice(
            &if let Some(place) = model.r#where {
                format!(
                    "Kumi talks to {}, in {name} {place}. /model changes it.{}",
                    model.name,
                    chosen.note.map(|n| format!(" {n}")).unwrap_or_default()
                )
            } else {
                format!("Kumi talks to {}, {name}'s first choice. /model changes it.", model.name)
            },
            NoticeTone::Info,
        );
    }
    pub(super) async fn check_model(&self) -> Result<(), RuntimeError> {
        let Some(models) = &self.0.options.models else {
            return Ok(());
        };
        if !self.0.state.borrow().overlays.is_empty() || self.0.state.borrow().closing {
            return Ok(());
        }
        let current = models.current();
        if current.model.is_none() {
            let chosen = models.choose_default().await?;
            if self.0.state.borrow().closing {
                return Ok(());
            }
            if let Some(chosen) = chosen {
                self.told_default(chosen);
                return Ok(());
            }
            self.notice("Sign in to a provider to talk to its models: ChatGPT with your plan, or others with an API key. Or open Ollama or LM Studio to use models on this computer.",NoticeTone::Info);
            if self.0.state.borrow().overlays.is_empty() {
                self.open_models().await?;
            }
            return Ok(());
        }
        let status = models.providers().await?.into_iter().find(|p| Some(p.id.as_str()) == current.provider.as_deref());
        if let Some(status) = &status {
            if !status.signed_in {
                self.offer_fix(FailureKind::Auth, Some(status.id.as_str()));
                return Ok(());
            }
        } else if let Some(server) = models.local().await.into_iter().find(|s| Some(&s.id) == current.provider.as_ref()) {
            if !server.running && !self.0.state.borrow().closing {
                let model = current.model.unwrap();
                self.notice(
                    &format!(
                        "{} isn't running, so {} can't answer yet. {}.",
                        server.name,
                        current.name.as_deref().unwrap_or_else(|| model.split_once('/').map(|(_, m)| m).unwrap_or(&model)),
                        server.start.as_deref().unwrap_or("Start it")
                    ),
                    NoticeTone::Info,
                );
                return Ok(());
            }
        }
        // A saved model the provider's own list no longer shows (retired from a plan's sign-in, say): said once, with the
        // list to choose another from (unless the producer is already typing or talking), before a message fails on it.
        if let Some(said) = crate::models::missing_model(models.as_ref()).await {
            if self.0.state.borrow().closing {
                return Ok(());
            }
            self.notice(&format!("{said}; choose another model."), NoticeTone::Warn);
            let free = {
                let state = self.0.state.borrow();
                state.overlays.is_empty() && state.editor.is_empty()
            };
            if free && !self.busy() {
                self.open_models().await?;
            }
        }
        Ok(())
    }
    pub(super) fn action<F, Fut>(&self, f: F) -> Action
    where
        F: Fn(TuiApp) -> Fut + 'static,
        Fut: Future<Output = Result<(), RuntimeError>> + 'static,
    {
        let weak = Rc::downgrade(&self.0);
        Rc::new(move || match weak.upgrade() {
            Some(a) => f(TuiApp(a)).boxed_local(),
            None => async { Ok(()) }.boxed_local(),
        })
    }
    pub(super) fn pick<F, Fut>(&self, picker: Picker, f: F) -> Rc<RefCell<Picker>>
    where
        F: Fn(TuiApp, PickerItem) -> Fut + 'static,
        Fut: Future<Output = Result<(), RuntimeError>> + 'static,
    {
        let picker = Rc::new(RefCell::new(picker));
        let weak = Rc::downgrade(&self.0);
        let choose: Choice = Rc::new(move |item| match weak.upgrade() {
            Some(a) => f(TuiApp(a), item).boxed_local(),
            None => async { Ok(()) }.boxed_local(),
        });
        self.open_panel(Panel::Pick { picker: picker.clone(), choose, choosing: Rc::default() });
        picker
    }
    /// Kumi's answer asked the producer to pick: its options, picked by number (or ↑↓) and sent with
    /// Enter. Other typing goes to the input box, so a free answer still works.
    pub(super) fn open_answers(&self, (question, mut choices): (String, Vec<String>)) {
        // The option Kumi recommends (marked by the model) is named, chosen to start with, and taken when nobody's there.
        let recommended = super::super::choices::recommended(&mut choices);
        let items = choices
            .iter()
            .enumerate()
            .map(|(at, choice)| {
                let item = PickerItem::new(format!("{}. {choice}", at + 1), choice);
                if Some(at) == recommended {
                    noted(item, "recommended", NoteTone::Accent)
                } else {
                    item
                }
            })
            .collect();
        let options = PickerOptions { answers: true, ..Default::default() };
        let taken = recommended.map(|at| (at, choices[at].clone()));
        let offered = Rc::new((question, choices));
        let picker = self.pick(Picker::with_options("Your answer", items, options), move |app, item| {
            let offered = offered.clone();
            async move {
                app.close_panel();
                let Some(words) = item.value else { return Ok(()) };
                // A choice the producer made, not the default Enter takes, is a pick to learn from.
                if item.chosen {
                    if let Some(index) = offered.1.iter().position(|choice| *choice == words) {
                        let (question, options) = (offered.0.clone(), offered.1.clone());
                        app.session().tell(op::Pick { pick: Picked::Answer { question, options, index } }.into());
                    }
                }
                app.0.state.borrow_mut().editor.set(&words);
                app.submit().await
            }
        });
        if let Some((at, value)) = taken {
            picker.borrow_mut().select(Some(&value));
            self.answer_for_producer(picker, at, value);
        }
    }
    /// Nobody's there to pick (a /goal, /loop or sound match ran this answer, or Kumi's window has been behind another
    /// for two minutes): Kumi takes its recommended option after `answerAfter` seconds, counting down in the list. Any
    /// key stops the count; the producer picks as ever.
    fn answer_for_producer(&self, picker: Rc<RefCell<Picker>>, at: usize, value: String) {
        let seconds = self.0.options.away.answer_after;
        let unattended = {
            let state = self.0.state.borrow();
            state.run_active || state.unfocused_since.is_some_and(|since| perf_now() - since >= UNATTENDED_MS)
        };
        if seconds == 0 || !unattended {
            return;
        }
        let weak = Rc::downgrade(&self.0);
        let task = tokio::task::spawn_local(async move {
            for left in (1..=seconds).rev() {
                {
                    let Some(a) = weak.upgrade() else { return };
                    let app = TuiApp(a);
                    if !app.same_picker(&picker) {
                        return;
                    }
                    picker.borrow_mut().options.hint = Some(format!("Taking {} (recommended) in {left} s · any key cancels", at + 1));
                    app.0.scheduler.request();
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let Some(a) = weak.upgrade() else { return };
            let app = TuiApp(a);
            let choose = app
                .top_panel(|panel| match panel {
                    Panel::Pick { picker: active, choose, choosing } if Rc::ptr_eq(active, &picker) && !choosing.get() => {
                        choosing.set(true);
                        Some((choose.clone(), choosing.clone()))
                    }
                    _ => None,
                })
                .flatten();
            app.0.state.borrow_mut().answer_countdown = None;
            let Some((choose, choosing)) = choose else { return };
            picker.borrow_mut().select(Some(&value));
            let item = picker.borrow().selected().cloned().map(|item| PickerItem { chosen: false, ..item });
            let Some(item) = item else { return };
            app.notice(&format!("No answer in {seconds} s; went with {}.", at + 1), NoticeTone::Info);
            // A goal that stopped on this question carries on with the answer, within its budget and its checks.
            let goal_waits = app
                .0
                .state
                .borrow()
                .objective
                .as_ref()
                .is_some_and(|goal| goal.status.state == ObjectiveState::Paused && goal.status.verdict == Some(Verdict::Blocked));
            let chosen = if goal_waits && app.can().goal {
                app.close_panel();
                app.0.state.borrow_mut().editor.set(&format!("/goal answer: {value}"));
                app.submit().await
            } else {
                choose(item).await
            };
            choosing.set(false);
            if let Err(error) = chosen {
                app.panel_failed(&error);
            }
        });
        if let Some(old) = self.0.state.borrow_mut().answer_countdown.replace(task) {
            old.abort();
        }
    }
    /// The producer pressed a key: Kumi doesn't answer for them.
    pub(super) fn stop_answer_countdown(&self) {
        let Some(task) = self.0.state.borrow_mut().answer_countdown.take() else { return };
        task.abort();
        self.top_panel(|panel| {
            if let Panel::Pick { picker, .. } = panel {
                picker.borrow_mut().options.hint = None;
            }
        });
        self.0.scheduler.request();
    }
    /// After an answer, whether to keep the technique from what Kumi built: 1 keeps it, 2 doesn't. Like
    /// an answer's own options, it opens only when nothing else is open, typed or waiting, and never over
    /// a question the answer ends on; other typing goes to the input box, and moving on leaves it unkept
    /// unless the next message says yes. True when it opened.
    pub(super) fn offer_technique(&self, name: &str) -> bool {
        let free = {
            let state = self.0.state.borrow();
            let asked = state
                .transcript
                .entries
                .iter()
                .rev()
                .find_map(|entry| match &*entry.borrow() {
                    Entry::Assistant { text, .. } => Some(text.trim_end().ends_with(['?', '？'])),
                    _ => None,
                })
                .unwrap_or(false);
            !asked && state.held.is_empty() && state.editor.is_empty() && state.overlays.is_empty()
        };
        if !free || !self.can().answer_technique {
            return false;
        }
        let items = vec![PickerItem::new("1. Yes, keep it", "yes"), PickerItem::new("2. No", "no")];
        let title = format!("Keep “{}” as a technique?", self.clean(&name.replace('\n', " "), 60));
        let mut picker = Picker::with_options(title, items, PickerOptions { answers: true, ..Default::default() });
        // A bare Enter is a no: only 1 (or moving to it) keeps it.
        picker.select(Some("no"));
        let name = name.to_owned();
        let picker = self.pick(picker, move |app, item| {
            let name = name.clone();
            async move {
                app.0.state.borrow_mut().offer = None;
                app.close_panel();
                let keep = item.value.as_deref() == Some("yes");
                // 1 or 2 is the producer's answer; a bare Enter (no) is the default.
                if item.chosen {
                    app.session().tell(op::Pick { pick: Picked::Technique { name, keep } }.into());
                }
                if !app.session().op(op::AnswerTechnique { keep }).await? && keep {
                    app.notice("That technique isn't waiting any more: its build was undone or its tracks deleted.", NoticeTone::Info);
                }
                Ok(())
            }
        });
        self.0.state.borrow_mut().offer = Some(picker);
        true
    }
    /// Whether `picker` is the technique offer's, letting go of the offer whatever is closing: closing that is a no.
    fn closing_offer(&self, picker: Option<&Rc<RefCell<Picker>>>) -> bool {
        let offer = self.0.state.borrow_mut().offer.take();
        offer.zip(picker).is_some_and(|(offer, picker)| Rc::ptr_eq(picker, &offer))
    }
    /// Whether `picker` is the question about another Set's conversation, letting go of it whatever is closing: closing
    /// that leaves it.
    fn closing_set_ask(&self, picker: Option<&Rc<RefCell<Picker>>>) -> bool {
        let ask = self.0.state.borrow_mut().set_ask.take();
        ask.zip(picker).is_some_and(|(ask, picker)| Rc::ptr_eq(picker, &ask))
    }
    /// Whether the top panel is `picker`'s list.
    fn same_picker(&self, picker: &Rc<RefCell<Picker>>) -> bool {
        self.top_panel(|panel| matches!(panel, Panel::Pick { picker: active, .. } if Rc::ptr_eq(active, picker))).unwrap_or(false)
    }
    /// What `look` makes of the top overlay, when it's one of the panels.
    pub(super) fn top_panel<R>(&self, look: impl FnOnce(&Panel) -> R) -> Option<R> {
        let top = self.0.state.borrow().overlays.top()?;
        let top = top.borrow();
        let seen = top.downcast_ref::<Panel>().map(look);
        seen
    }
    /// Whether Kumi's numbered answers are the top overlay, waiting for the producer's pick.
    pub(super) fn answers_open(&self) -> bool {
        self.top_panel(|panel| matches!(panel, Panel::Pick { picker, .. } if picker.borrow().options.answers)).unwrap_or(false)
    }
    /// Whether a sign-in is the top overlay: a key's box, or ChatGPT's.
    pub(super) fn signing_in(&self) -> bool {
        self.top_panel(|panel| matches!(panel, Panel::Key { .. } | Panel::ChatGpt { .. })).unwrap_or(false)
    }
    /// Opens `panel` over the conversation in place of the top one, which isn't told it closed: a panel opening over
    /// another has always replaced it.
    pub(super) fn open_panel(&self, panel: Panel) -> PanelRef {
        let panel = Rc::new(RefCell::new(panel));
        self.0.state.borrow_mut().overlays.open(panel.clone());
        self.0.scheduler.request();
        panel
    }
    /// Closes the top overlay.
    pub(super) fn close_panel(&self) {
        let top = self.0.state.borrow().overlays.top();
        match top {
            Some(top) => self.close_overlay(&top),
            None => self.0.scheduler.request(),
        }
    }
    /// Closes `overlay` while it's open, and it lets go of what it holds.
    fn close_overlay<T: Overlay<TuiApp> + ?Sized>(&self, overlay: &Rc<RefCell<T>>) {
        if self.0.state.borrow_mut().overlays.remove(overlay) {
            overlay.borrow_mut().closed(self);
        }
        self.0.scheduler.request();
    }
    pub(super) fn panel_failed(&self, error: &RuntimeError) {
        self.close_panel();
        if !self.0.state.borrow().closing {
            self.error(error);
        }
    }
    /// Input while something's open: the top overlay takes it, then stays, closes, or closes and hands it on.
    pub(super) fn panel_input(&self, event: InputEvent) {
        let Some(top) = self.0.state.borrow().overlays.top() else {
            return;
        };
        let handled = top.borrow_mut().handle(self, event);
        match handled {
            Handled::Kept => {}
            Handled::Close => self.close_overlay(&top),
            Handled::Pass(event) => {
                self.close_overlay(&top);
                self.on_input(event);
            }
        }
    }
    /// A key, typed text or a paste for a list or a sign-in.
    fn panel_key(&self, panel: &mut Panel, event: InputEvent) -> Handled {
        if let InputEvent::Key { name, mods, .. } = &event {
            if name == "escape" || (mods.ctrl && name == "c") {
                let picker = match panel {
                    Panel::Pick { picker, .. } => Some(&*picker),
                    _ => None,
                };
                let declined = self.closing_offer(picker);
                let left = self.closing_set_ask(picker);
                if declined {
                    self.task(|app| async move { app.session().op(op::AnswerTechnique { keep: false }).await.map(|_| ()) });
                }
                if left {
                    self.task(|app| async move { app.session().op(op::AnswerSet { choice: SetChoice::Cancel }).await });
                }
                return Handled::Close;
            }
            match panel {
                Panel::Pick { picker, choose, choosing } => match name.as_str() {
                    "up" => picker.borrow_mut().r#move(-1),
                    "tab" if picker.borrow().tabs.is_some() => picker.borrow_mut().next_tab(),
                    "down" | "tab" => picker.borrow_mut().r#move(1),
                    "backspace" => picker.borrow_mut().erase(),
                    // A second Enter before the first choice is made would make it again (an answer sent twice).
                    // Ctrl-F chooses the other way, where the list has one (/model: the fallback).
                    "enter" | "f" if !choosing.get() && (name == "enter" || mods.ctrl && picker.borrow().options.alternate) => {
                        let item = {
                            let picker = picker.borrow();
                            let item = picker.selected().cloned().map(|item| PickerItem { chosen: picker.chosen, ..item });
                            if name == "f" {
                                item.filter(|item| item.value.as_deref().is_some_and(|value| !value.contains("signin:")))
                                    .map(|item| PickerItem { value: item.value.map(|value| format!("fallback:{value}")), ..item })
                            } else {
                                item
                            }
                        };
                        if let Some(item) = item {
                            choosing.set(true);
                            let (choose, choosing) = (choose.clone(), choosing.clone());
                            self.task(move |app| async move {
                                let chosen = choose(item).await;
                                choosing.set(false);
                                if let Err(error) = chosen {
                                    app.panel_failed(&error);
                                }
                                Ok(())
                            });
                        }
                    }
                    _ => {}
                },
                Panel::Key { secret, checking, .. } if !*checking => {
                    if name == "backspace" {
                        let count = secret.encode_utf16().count().saturating_sub(1);
                        *secret = head(secret, count);
                    } else if mods.ctrl && name == "u" {
                        secret.clear();
                    } else if name == "enter" && !secret.is_empty() {
                        // The check holds on to this panel: the top one, as only the top one takes keys.
                        let me = self.0.state.borrow().overlays.top();
                        if let Some(me) = me {
                            self.task(move |app| async move {
                                app.submit_key(me).await;
                                Ok(())
                            });
                        }
                    }
                }
                _ => {}
            }
            return Handled::Kept;
        }
        let text = match &event {
            InputEvent::Text { text } | InputEvent::Paste { text } => text.clone(),
            _ => return Handled::Kept,
        };
        match panel {
            Panel::ChatGpt { url, code, by_code, .. } if text.to_lowercase() == "c" => {
                let copied =
                    if *by_code { code.as_ref().map(|code| ("code", code.code.clone())) } else { url.clone().map(|url| ("link", url)) };
                if let Some((what, text)) = copied {
                    self.copy_text(&text);
                    self.notice(&format!("Copied the sign-in {what}."), NoticeTone::Info);
                }
            }
            // No browser here after all, or no code (over SSH, where the browser's link can still be forwarded): the other
            // way, with what was to follow the sign-in, in this one's place.
            Panel::ChatGpt { by_code, abort, then, .. } if text.to_lowercase() == if *by_code { "b" } else { "d" } => {
                let (then, by_code) = (then.take(), !*by_code);
                abort.cancel();
                self.sign_in_chatgpt(by_code, then);
            }
            Panel::Pick { picker, .. } => {
                if picker.borrow().options.answers {
                    // A number picks its option and Enter sends it; typing on ("2 dB quieter") takes the number
                    // into the input box with the rest, so a free answer that starts with one arrives whole.
                    let picked = {
                        let picker = picker.borrow();
                        let number = text.parse().ok().filter(|_| picker.typed.is_empty());
                        number.and_then(|n| picker.numbered(n)).and_then(|item| item.value.clone())
                    };
                    if let Some(value) = picked {
                        let mut picker = picker.borrow_mut();
                        picker.select(Some(&value));
                        picker.typed = text;
                        picker.chosen = true;
                        return Handled::Kept;
                    }
                    let typed = std::mem::take(&mut picker.borrow_mut().typed);
                    // Typed on, it's words of the answer, never a key: a `?` that starts one doesn't open the help.
                    if matches!(event, InputEvent::Text { .. }) {
                        self.type_into_box(&format!("{typed}{text}"), true);
                        return Handled::Close;
                    }
                    self.0.state.borrow_mut().editor.insert(&typed);
                    return Handled::Pass(event);
                }
                if picker.borrow().filter.is_empty() && text.starts_with('/') {
                    return Handled::Pass(event);
                }
                picker.borrow_mut().r#type(&text);
            }
            Panel::Key { secret, checking, status, .. } if !*checking => {
                let clean: String =
                    text.chars().filter(|c| !string::trim(&c.to_string()).is_empty() && (*c as u32) > 31 && *c != '\x7f').collect();
                *secret = head(&(secret.clone() + &clean), 4096);
                *status = None;
            }
            _ => {}
        }
        Handled::Kept
    }
    fn aside_input(&self, panel: &mut Panel, event: InputEvent) -> Handled {
        let Panel::Btw { at, scroll } = panel else { return Handled::Kept };
        if let InputEvent::Key { name, mods, .. } = &event {
            if matches!(name.as_str(), "escape" | "enter") || (mods.ctrl && name == "c") {
                return Handled::Close;
            }
            match name.as_str() {
                "up" | "pageup" => *scroll = (*scroll - if name == "up" { 1 } else { 5 }).max(0),
                "down" | "pagedown" => *scroll += if name == "down" { 1 } else { 5 },
                "left" if *at > 0 => {
                    *at -= 1;
                    *scroll = 0;
                }
                "right" if *at + 1 < self.0.state.borrow().asides.len() => {
                    *at += 1;
                    *scroll = 0;
                }
                _ => {}
            }
            return Handled::Kept;
        }
        if let InputEvent::Text { text } = event {
            if text == " " {
                return Handled::Close;
            }
            if text.to_lowercase() == "c" {
                let answer =
                    self.0.state.borrow().asides.get(*at).map(|a| string::trim(&a.borrow().answer).to_string()).unwrap_or_default();
                if !answer.is_empty() {
                    self.0.tty.write(&format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(answer)));
                    self.notice("Copied the side answer.", NoticeTone::Info);
                }
            }
        }
        Handled::Kept
    }
    pub(super) fn model_label(&self) -> Option<String> {
        let current = self.0.options.models.as_ref()?.current();
        // The fallback, while it answers for the producer's model.
        if let Some(answering) = self.session().answering_fallback() {
            let name = answering.split_once('/').map_or(answering.as_str(), |(_, name)| name).to_string();
            return Some(format!("{name} · fallback"));
        }
        let Some(model) = current.model else {
            return Some("no model chosen".into());
        };
        let name = current.name.unwrap_or_else(|| model.split_once('/').map(|(_, m)| m).unwrap_or(&model).into());
        let label = current.effort.map(|e| format!("{name} · {}", e.as_str())).unwrap_or(name);
        Some(current.fast.map(|tier| format!("{label} · {}", tier.name.to_lowercase())).unwrap_or(label))
    }
    /// How much of the plan behind the answering model is used, near its limit only: "plan 82% · resets 14:20".
    pub(super) fn plan_line(&self) -> Option<(String, bool)> {
        let answering = self.session().answering_fallback();
        let provider = match answering {
            Some(model) => model.split('/').next().map(str::to_string),
            None => self.0.options.models.as_ref()?.current().provider,
        }?;
        let window = kumi_ai::providers::usage::nearest(&provider, 80.)?;
        let resets = window.resets_at_ms.map(|at| format!(" · resets {}", reset_words(at, now_ms_f64()))).unwrap_or_default();
        Some((format!("plan {}%{resets}", window.used_percent.round()), window.used_percent >= 95.))
    }
    pub(super) fn tokens_used(&self) -> String {
        let Some(provider) = self.0.options.models.as_ref().and_then(|m| m.cloud_provider(&m.current().provider?)) else {
            return String::new();
        };
        let state = self.0.state.borrow();
        if provider.row().sign_in != SignIn::ApiKey || state.used.answers == 0 {
            return String::new();
        }
        let count = |n: f64| {
            if n < 1000. {
                number::to_string(n)
            } else if n < 1000000. {
                format!("{}k", number::to_fixed(n / 1000., 1))
            } else {
                format!("{}M", number::to_fixed(n / 1000000., 2))
            }
        };
        format!(
            " · this session: {} tokens in{}, {} out",
            count(state.used.input),
            if state.used.cached != 0. { format!(" ({} cached)", count(state.used.cached)) } else { String::new() },
            count(state.used.output)
        )
    }
    pub(super) async fn open_models(&self) -> Result<(), RuntimeError> {
        self.open_model_list(false).await
    }
    /// The same list, to choose the model that carries on while the chosen one can't; the message that stopped is
    /// sent again once one is chosen.
    pub(super) async fn open_fallbacks(&self) -> Result<(), RuntimeError> {
        self.open_model_list(true).await
    }
    async fn open_model_list(&self, fallback: bool) -> Result<(), RuntimeError> {
        let Some(models) = self.0.options.models.clone() else {
            return Ok(());
        };
        let options = if fallback {
            PickerOptions { filterable: true, ..Default::default() }
        } else {
            PickerOptions {
                filterable: true,
                alternate: true,
                hint: Some("↑↓ to move · enter to choose · ctrl+f sets the fallback · esc to close".into()),
                ..Default::default()
            }
        };
        let picker = self.pick(
            Picker::with_options(
                if fallback { "Choose a fallback model" } else { "Choose a model" },
                vec![inert("Reading your sign-ins…")],
                options,
            ),
            move |app, item| async move {
                if fallback {
                    app.choose_fallback(item, true).await
                } else {
                    app.choose_model_item(item).await
                }
            },
        );
        let m = models.clone();
        let servers = tokio::task::spawn_local(async move { m.local().await });
        let statuses = Rc::new(models.providers().await?);
        let chosen = models.fallback();
        let current = if fallback { chosen.clone() } else { models.current().model };
        let lists = Rc::new(RefCell::new(HashMap::<String, Listed>::new()));
        let local = Rc::new(RefCell::new(None::<Vec<LocalStatus>>));
        let weak = Rc::downgrade(&self.0);
        let update: Rc<dyn Fn()> = {
            let picker = picker.clone();
            let statuses = statuses.clone();
            let current = current.clone();
            let lists = lists.clone();
            let local = local.clone();
            Rc::new(move || {
                if let Some(a) = weak.upgrade() {
                    let app = TuiApp(a);
                    if app.same_picker(&picker) {
                        let mut items =
                            model_items(&statuses, &lists.borrow(), local.borrow().as_deref(), current.as_deref(), chosen.as_deref());
                        if fallback && chosen.is_some() {
                            items.insert(0, item("No fallback", "fallback:", "wait for the limit to reset instead"));
                        }
                        picker.borrow_mut().set_items(items);
                        picker.borrow_mut().select(current.as_deref());
                        app.0.scheduler.request();
                    }
                }
            })
        };
        update();
        let mut jobs: Vec<LocalBoxFuture<'static, ()>> = vec![];
        for status in statuses.iter().filter(|p| p.signed_in) {
            let id = status.id.as_str().to_string();
            jobs.push(read_models(models.clone(), id, lists.clone(), update.clone()));
        }
        jobs.push(
            async move {
                let found = servers.await.unwrap_or_default();
                *local.borrow_mut() = Some(found.clone());
                update();
                futures::future::join_all(
                    found.into_iter().filter(|s| s.running).map(|s| read_models(models.clone(), s.id, lists.clone(), update.clone())),
                )
                .await;
            }
            .boxed_local(),
        );
        futures::future::join_all(jobs).await;
        Ok(())
    }
    /// Makes `item`'s model the fallback (`"fallback:"` alone clears it; choosing the fallback again in /model
    /// with ctrl+f clears it too), then sends the last message again when it was a usage limit that stopped it.
    async fn choose_fallback(&self, item: PickerItem, resend: bool) -> Result<(), RuntimeError> {
        let models = self.0.options.models.clone().unwrap();
        let value = item.value.clone().unwrap_or_default();
        if let Some(provider) = value.strip_prefix("signin:").and_then(ProviderId::parse) {
            self.sign_in(provider, Some(self.action(|app| async move { app.open_fallbacks().await })));
            return Ok(());
        }
        if value == "login:" {
            return self.open_login().await;
        }
        let model = value.strip_prefix("fallback:").unwrap_or(&value);
        let toggled_off = !resend && models.fallback().as_deref() == Some(model);
        let next = (!model.is_empty() && !toggled_off).then_some(model);
        models.set_fallback(next).await?;
        self.close_panel();
        let name = |id: &str| id.split_once('/').map_or(id, |(_, name)| name).to_string();
        let current = models.current().model.map(|id| name(&id)).unwrap_or_else(|| "your model".into());
        match next {
            Some(model) => self.notice(
                &format!(
                    "{} carries on from your next message whenever {current}'s plan limit is reached or its provider is down.",
                    name(model)
                ),
                NoticeTone::Info,
            ),
            None => self.notice("No fallback: when the plan's limit is reached, Kumi waits for it to reset.", NoticeTone::Info),
        }
        if resend && next.is_some() {
            self.resend();
        }
        Ok(())
    }
    async fn choose_model_item(&self, item: PickerItem) -> Result<(), RuntimeError> {
        let models = self.0.options.models.as_ref().unwrap();
        let value = item.value.as_deref().unwrap_or("");
        if let Some(provider) = value.strip_prefix("signin:").and_then(ProviderId::parse) {
            self.sign_in(provider, Some(self.action(|app| async move { app.open_models().await })));
            return Ok(());
        }
        if value == "login:" {
            return self.open_login().await;
        }
        if value.starts_with("fallback:") {
            return self.choose_fallback(item, false).await;
        }
        let note = match models.choose(value).await {
            Ok(note) => note,
            Err(error) => {
                if let Some(provider) = error
                    .kumi()
                    .filter(|e| e.kind == FailureKind::Auth)
                    .and_then(|e| e.provider.as_deref())
                    .and_then(|p| models.cloud_provider(p))
                {
                    self.sign_in(
                        provider,
                        Some(self.action(move |app| {
                            let item = item.clone();
                            async move { app.choose_model_item(item).await }
                        })),
                    );
                    return Ok(());
                }
                return Err(error);
            }
        };
        self.close_panel();
        let current = models.current();
        let effort = if let Some(e) = current.effort {
            format!(", at {} effort", e.as_str())
        } else if let Some(e) = current.default_effort {
            format!(", at its usual {} effort", e.as_str())
        } else {
            String::new()
        };
        self.notice(
            &format!(
                "Kumi talks to {} from your next message{effort}.{}{}{}",
                current.name.as_deref().unwrap_or(value),
                if current.pinned { " KUMI_MODEL is set, so this lasts until Kumi closes." } else { "" },
                note.map(|n| format!(" {n}")).unwrap_or_default(),
                fast_note(current.fast.as_ref())
            ),
            NoticeTone::Info,
        );
        Ok(())
    }
    pub(super) async fn open_effort(&self) -> Result<(), RuntimeError> {
        let models = self.0.options.models.as_ref().unwrap();
        let mut current = models.current();
        if current.model.is_none() {
            return self.open_models().await;
        }
        if let Some(provider) = &current.provider {
            let _ = models.models(provider, false).await;
        }
        current = models.current();
        let name = current.name.unwrap_or_else(|| current.model.unwrap());
        if current.efforts.is_empty() {
            self.notice(&format!("{name} has no effort setting to choose."), NoticeTone::Info);
            return Ok(());
        }
        let mut items = vec![item(
            current.default_effort.map(|e| format!("Default ({})", e.as_str())).unwrap_or_else(|| "Default".into()),
            "default",
            "The model's own setting",
        )];
        if current.effort.is_none() {
            items[0] = noted(items[0].clone(), "current", NoteTone::Accent);
        }
        items.extend(current.efforts.iter().map(|level| {
            let description = level.description.clone().unwrap_or_else(|| {
                match level.effort {
                    Effort::None => "No thinking; answers at once",
                    Effort::Minimal => "Barely thinks",
                    Effort::Low => "Quick; lighter thinking",
                    Effort::Medium => "Balanced",
                    Effort::High => "Thorough",
                    Effort::Xhigh => "More thorough still",
                    Effort::Max => "As hard as it can",
                }
                .into()
            });
            let item = item(level.effort.as_str(), level.effort.as_str(), description);
            if Some(level.effort) == current.effort {
                noted(item, "current", NoteTone::Accent)
            } else {
                item
            }
        }));
        let mut picker = Picker::new(format!("How hard {name} thinks · lower answers sooner"), items);
        picker.select(Some(current.effort.map(Effort::as_str).unwrap_or("default")));
        self.pick(picker, move |app, item| {
            let name = name.clone();
            async move {
                let effort = item.value.as_deref().and_then(Effort::parse);
                app.0.options.models.as_ref().unwrap().set_effort(effort).await?;
                app.close_panel();
                app.notice(
                    &if let Some(e) = effort {
                        format!("{name} thinks at {} effort from your next message.", e.as_str())
                    } else {
                        format!("{name} uses its own effort from your next message.")
                    },
                    NoticeTone::Info,
                );
                Ok(())
            }
        });
        Ok(())
    }
    pub(super) async fn open_login(&self) -> Result<(), RuntimeError> {
        let statuses = self.0.options.models.as_ref().unwrap().providers().await?;
        let items = statuses
            .iter()
            .map(|p| {
                noted(
                    item(&p.name, p.id.as_str(), if p.sign_in == SignIn::Chatgpt { "with your ChatGPT plan" } else { "with an API key" }),
                    if p.via.as_deref() == Some("environment") {
                        format!("key from {}", p.key_env.as_deref().unwrap_or("undefined"))
                    } else if p.signed_in {
                        "signed in".into()
                    } else {
                        "sign in".into()
                    },
                    if p.signed_in { NoteTone::Faint } else { NoteTone::Accent },
                )
            })
            .collect();
        let options = PickerOptions { filterable: true, ..Default::default() };
        self.pick(Picker::with_options("Sign in to", items, options), |app, item| async move {
            if let Some(p) = item.value.as_deref().and_then(ProviderId::parse) {
                app.sign_in(p, None);
            }
            Ok(())
        });
        Ok(())
    }
    pub(super) async fn open_logout(&self) -> Result<(), RuntimeError> {
        let statuses = self.0.options.models.as_ref().unwrap().providers().await?.into_iter().filter(|p| p.signed_in).collect::<Vec<_>>();
        if statuses.is_empty() {
            self.notice("You're not signed in to any provider.", NoticeTone::Info);
            return Ok(());
        }
        let items = statuses
            .iter()
            .map(|p| {
                let mut item = item(
                    &p.name,
                    p.id.as_str(),
                    if p.via.as_deref() == Some("environment") {
                        format!("Its key comes from {}; unset it to sign out", p.key_env.as_deref().unwrap_or("undefined"))
                    } else if p.via.as_deref() == Some("chatgpt") {
                        "Your ChatGPT sign-in".into()
                    } else {
                        "The key saved in Kumi".into()
                    },
                );
                item.inert = p.via.as_deref() == Some("environment");
                item
            })
            .collect();
        self.pick(Picker::new("Sign out of", items), |app, item| async move {
            let Some(provider) = item.value.as_deref().and_then(ProviderId::parse) else {
                return Ok(());
            };
            let info = provider.row();
            let shared = info.shared_by.map(|names| format!(" ({names} share it)")).unwrap_or_default();
            app.pick(
                Picker::new(
                    format!("Sign out of {}?", info.name),
                    vec![
                        super::panels::item("Sign out", "yes", format!("Kumi forgets this sign-in{shared}")),
                        PickerItem::new("Keep it", "no"),
                    ],
                ),
                move |app, answer| async move {
                    app.close_panel();
                    if answer.value.as_deref() != Some("yes") {
                        return Ok(());
                    }
                    let models = app.0.options.models.as_ref().unwrap();
                    let removed = models.sign_out(provider).await?;
                    let status = models.providers().await?.into_iter().find(|p| p.id == provider);
                    let still = if status.as_ref().and_then(|p| p.via.as_deref()) == Some("environment") {
                        let variable = status.and_then(|p| p.key_env).or(info.first_key_env().map(str::to_string));
                        format!(" {} is still set, so Kumi uses that key now.", variable.as_deref().unwrap_or("undefined"))
                    } else {
                        String::new()
                    };
                    app.notice(
                        &format!(
                            "{}{still}",
                            if removed {
                                format!("Signed out of {}.", info.name)
                            } else {
                                format!("Kumi had no sign-in for {} to remove.", info.name)
                            }
                        ),
                        NoticeTone::Info,
                    );
                    Ok(())
                },
            );
            Ok(())
        });
        Ok(())
    }
    pub(super) async fn open_update(&self) -> Result<(), RuntimeError> {
        let updates = self.0.options.updates.as_ref().unwrap();
        if self.busy() {
            self.notice("Kumi is working: /update once it's done, or press esc to stop it first.", NoticeTone::Info);
            return Ok(());
        }
        let mut latest = self.0.state.borrow().newer.clone();
        if latest.is_none() {
            self.notice("Looking for a newer Kumi…", NoticeTone::Info);
            latest = match (updates.check)().await {
                Ok(latest) => latest,
                Err(error) => {
                    self.notice(
                        &format!(
                            "{}. Try /update again later.",
                            safe_error_message(Some(&error.message()), &self.0.state.borrow().secrets)
                        ),
                        NoticeTone::Warn,
                    );
                    return Ok(());
                }
            };
            if latest.is_none() {
                self.notice(&format!("Kumi is up to date ({}).", updates.current), NoticeTone::Info);
                return Ok(());
            }
            self.0.state.borrow_mut().newer = latest.clone();
        }
        self.pick(
            Picker::new(
                format!("Update to Kumi {}?", latest.unwrap()),
                vec![item("Update now", "yes", "Kumi closes, updates and opens again"), PickerItem::new("Not now", "no")],
            ),
            |app, answer| async move {
                app.close_panel();
                if answer.value.as_deref() == Some("yes") {
                    (app.0.options.updates.as_ref().unwrap().request)();
                    app.reopen().await;
                }
                Ok(())
            },
        );
        Ok(())
    }
    pub(super) fn insert_spoken(&self, text: &str) {
        let clean = self.clean(text, usize::MAX);
        let mut state = self.0.state.borrow_mut();
        let words = state.editor.text();
        let chars = graphemes(&words);
        let at = state.editor.cursor();
        let space = |s: Option<&&str>| s.is_some_and(|s| !s.chars().any(|c| string::trim(&c.to_string()).is_empty()));
        let text =
            format!("{}{clean}{}", if at > 0 && space(chars.get(at - 1)) { " " } else { "" }, if space(chars.get(at)) { " " } else { "" });
        state.editor.insert(&text);
        state.recall = None;
        state.menu_dismissed = false;
        drop(state);
        self.0.scheduler.request();
    }
    pub(super) fn spoken_names(&self) -> Vec<String> {
        let state = self.0.state.borrow();
        let focus = state.focus.as_ref();
        [
            state.set_name.as_ref(),
            focus.and_then(|f| f.track.as_ref()).map(|t| &t.name),
            focus.and_then(|f| f.device.as_ref()),
            focus.and_then(|f| f.item.as_ref()).and_then(|item| item.name.as_ref()),
            state.pinned.as_ref().map(|p| &p.pin.name),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect()
    }
    fn voice_items(&self) -> Vec<PickerItem> {
        let choices = self.0.options.voice.as_ref().unwrap().choices();
        vec![
            item(
                if self.0.voice.as_ref().unwrap().listening() { "Stop listening" } else { "Start listening" },
                "listen",
                "ctrl+t, or hold it while you talk",
            ),
            noted(
                item(
                    "Send when you stop",
                    "send",
                    if choices.send { "What you say is sent at once" } else { "What you say waits in the box for enter" },
                ),
                if choices.send { "on" } else { "off" },
                if choices.send { NoteTone::Accent } else { NoteTone::Faint },
            ),
            noted(item("Language", "language", "What you speak"), language_name(&choices.language), NoteTone::Faint),
            noted(
                item("Microphone", "microphone", "What Kumi listens through"),
                choices.microphone.unwrap_or_else(|| "system default".into()),
                NoteTone::Faint,
            ),
        ]
    }
    pub(super) fn open_voice(&self, select: Option<&str>) {
        let mut picker = Picker::new("Talk to Kumi", self.voice_items());
        picker.select(select);
        self.pick(picker, |app, item| async move {
            match item.value.as_deref() {
                Some("listen") => {
                    app.close_panel();
                    let voice = app.0.voice.as_ref().unwrap();
                    if voice.listening() {
                        voice.stop(false, None);
                    } else {
                        voice.start(false);
                    }
                }
                Some("send") => {
                    let control = app.0.options.voice.as_ref().unwrap();
                    control.choose(VoiceChange { send: Some(!control.choices().send), ..Default::default() })?;
                    app.top_panel(|panel| {
                        if let Panel::Pick { picker, .. } = panel {
                            picker.borrow_mut().set_items(app.voice_items());
                        }
                    });
                    app.0.scheduler.request();
                }
                Some("language") => app.open_voice_language(),
                _ => app.open_microphones().await?,
            }
            Ok(())
        });
    }
    fn open_voice_language(&self) {
        let control = self.0.options.voice.as_ref().unwrap();
        let current = control.choices().language;
        let mut items = vec![item("English", "en", "A speech model for English alone: the quickest and surest")];
        for code in [control.system_language(), current.clone()] {
            if code != "en" && code != "auto" && !items.iter().any(|i| i.value.as_ref() == Some(&code)) {
                items.push(item(language_name(&code), code, "The speech model for many languages, told to expect this one"));
            }
        }
        items.push(item("Any language", "auto", "The speech model for many languages finds the one you speak"));
        for i in &mut items {
            if i.value.as_ref() == Some(&current) {
                *i = noted(i.clone(), "current", NoteTone::Accent);
            }
        }
        self.pick(Picker::new("The language you speak", items), |app, item| async move {
            app.0.options.voice.as_ref().unwrap().choose(VoiceChange { language: item.value, ..Default::default() })?;
            app.open_voice(Some("language"));
            Ok(())
        });
    }
    async fn open_microphones(&self) -> Result<(), RuntimeError> {
        let control = self.0.options.voice.as_ref().unwrap();
        let current = control.choices().microphone.unwrap_or_default();
        let picker = self.pick(
            Picker::new("The microphone Kumi listens through", vec![inert("Looking for microphones…")]),
            |app, item| async move {
                app.0
                    .options
                    .voice
                    .as_ref()
                    .unwrap()
                    .choose(VoiceChange { microphone: Some(item.value.filter(|s| !s.is_empty())), ..Default::default() })?;
                app.open_voice(Some("microphone"));
                Ok(())
            },
        );
        let mut names = control.microphones().await.unwrap_or_default();
        if !self.same_picker(&picker) {
            return Ok(());
        }
        if !current.is_empty() && !names.contains(&current) {
            names.push(current.clone());
        }
        let mut items = vec![item(
            "System default",
            "",
            if cfg!(windows) { "The first one Windows lists" } else { "The input your sound settings choose" },
        )];
        items.extend(names.into_iter().map(|name| PickerItem::new(name.clone(), name)));
        for i in &mut items {
            if i.value.as_ref() == Some(&current) {
                *i = noted(i.clone(), "current", NoteTone::Accent);
            }
        }
        picker.borrow_mut().set_items(items);
        picker.borrow_mut().select(Some(&current));
        self.0.scheduler.request();
        Ok(())
    }
    pub(super) fn offer_voice_fix(&self, trouble: VoiceTrouble) {
        let Some(control) = &self.0.options.voice else {
            return;
        };
        if !self.0.state.borrow().overlays.is_empty() || self.0.state.borrow().closing {
            return;
        }
        let privacy = matches!(trouble, VoiceTrouble::Permission | VoiceTrouble::Silence | VoiceTrouble::Device) && control.has_privacy();
        let another = matches!(trouble, VoiceTrouble::Silence | VoiceTrouble::Device);
        if !privacy && !another {
            return;
        }
        let mut items = vec![];
        if privacy {
            items.push(PickerItem::new(
                if cfg!(target_os = "macos") { "Open Privacy & Security › Microphone" } else { "Open the microphone's privacy settings" },
                "privacy",
            ));
        }
        if another {
            items.push(PickerItem::new("Choose another microphone", "microphone"));
        }
        items.push(PickerItem::new("Not now", "later"));
        self.pick(
            Picker::new(if trouble == VoiceTrouble::Permission { "Let Kumi hear the microphone?" } else { "Fix the microphone?" }, items),
            |app, item| async move {
                app.close_panel();
                match item.value.as_deref() {
                    Some("privacy") => app.0.options.voice.as_ref().unwrap().open_privacy(),
                    Some("microphone") => app.open_microphones().await?,
                    _ => {}
                }
                Ok(())
            },
        );
    }
}
#[derive(Clone)]
enum Listed {
    Models(Vec<ModelInfo>),
    Refused,
    Unreadable,
}
fn read_models(
    models: Rc<dyn ModelController>,
    id: String,
    lists: Rc<RefCell<HashMap<String, Listed>>>,
    update: Rc<dyn Fn()>,
) -> LocalBoxFuture<'static, ()> {
    async move {
        let result = match models.models(&id, false).await {
            Ok(list) => Listed::Models(list),
            Err(error) if error.kumi().is_some_and(|e| e.kind == FailureKind::Auth) => Listed::Refused,
            Err(_) => Listed::Unreadable,
        };
        lists.borrow_mut().insert(id, result);
        update();
    }
    .boxed_local()
}
/// A model's line in /model: its description, then what to know before choosing it.
fn model_detail(model: &ModelInfo) -> String {
    let mut detail: Vec<String> = model.description.iter().cloned().collect();
    // A model on this computer says it in its description, in the DAW's words.
    if model.tools == Some(false) && model.r#where.is_none() {
        detail.push("chat only".into());
    }
    detail.extend(model.labels.iter().cloned());
    if model.status.as_deref() == Some("deprecated") {
        detail.push("deprecated".into());
    }
    if model.facts_from == Some(FactsFrom::Guess) {
        detail.push("unverified".into());
    }
    detail.join(" · ")
}
fn model_items(
    statuses: &[ProviderStatus],
    lists: &HashMap<String, Listed>,
    local: Option<&[LocalStatus]>,
    current: Option<&str>,
    fallback: Option<&str>,
) -> Vec<PickerItem> {
    let model_item = |m: &ModelInfo| {
        let i = item(&m.name, &m.id, model_detail(m));
        if Some(m.id.as_str()) == current {
            noted(i, "current", NoteTone::Accent)
        } else if Some(m.id.as_str()) == fallback {
            noted(i, "fallback", NoteTone::Faint)
        } else {
            i
        }
    };
    let mut items = vec![];
    // A provider added after the first five shows here once signed in; the rest wait in /login.
    let waiting: Vec<&str> = statuses.iter().filter(|p| !p.signed_in && !p.id.row().reserved).map(|p| p.name.as_str()).collect();
    for p in statuses.iter().filter(|p| p.signed_in || p.id.row().reserved) {
        let listed = lists.get(p.id.as_str());
        let refused = matches!(listed, Some(Listed::Refused));
        let environment = p.via.as_deref() == Some("environment");
        let note = if refused {
            if environment {
                format!("{} not accepted", p.key_env.as_deref().unwrap_or("undefined"))
            } else {
                "sign-in not accepted".into()
            }
        } else if environment {
            format!("key from {}", p.key_env.as_deref().unwrap_or("undefined"))
        } else if p.signed_in {
            "signed in".into()
        } else {
            "not signed in".into()
        };
        items.push(noted(PickerItem::heading(&p.name), note, if refused { NoteTone::Warn } else { NoteTone::Faint }));
        if !p.signed_in || refused {
            items.push(noted(
                item(
                    format!("Sign in to {}{}", p.name, if refused { " again" } else { "" }),
                    format!("signin:{}", p.id.as_str()),
                    if p.sign_in == SignIn::Chatgpt { "with your ChatGPT plan" } else { "with an API key" },
                ),
                "sign in",
                NoteTone::Accent,
            ));
        } else {
            match listed {
                None => items.push(inert("Reading its models…")),
                Some(Listed::Unreadable) => items.push(inert("Couldn't read its models just now; try /model again.")),
                Some(Listed::Models(list)) if list.is_empty() => items.push(inert("It lists no models for this sign-in.")),
                Some(Listed::Models(list)) => items.extend(list.iter().map(model_item)),
                _ => {}
            }
        }
    }
    if !waiting.is_empty() {
        let named = match waiting.as_slice() {
            [first, second, third, rest @ ..] if !rest.is_empty() => format!("{first}, {second}, {third} and {} more", rest.len()),
            [.., last] if waiting.len() > 1 => format!("{} and {last}", waiting[..waiting.len() - 1].join(", ")),
            _ => waiting.join(""),
        };
        items.push(noted(PickerItem::heading("More providers"), "not signed in", NoteTone::Faint));
        items.push(noted(item("Sign in to one", "login:", named), "sign in", NoteTone::Accent));
    }
    if let Some(local) = local {
        if local.is_empty() {
            items.extend([PickerItem::heading("On this computer"), inert("Open Ollama or LM Studio, and its models show here.")]);
        }
        for server in local {
            items.push(noted(
                PickerItem::heading(format!("{} · {}", server.name, server.r#where)),
                if server.running { "running" } else { "not running" },
                if server.running { NoteTone::Faint } else { NoteTone::Warn },
            ));
            if !server.running {
                items.push(inert(server.start.as_deref().unwrap_or("Start it, then /model again.")));
                continue;
            }
            match lists.get(&server.id) {
                None => items.push(inert("Reading its models…")),
                Some(Listed::Refused) => items.push(inert(if server.id == "lmstudio" {
                    "It wants an API token: set LM_API_TOKEN to one from its server settings."
                } else {
                    "It didn't accept Kumi's key for it (apiKey in ~/.kumi/settings.json)."
                })),
                Some(Listed::Unreadable) => items.push(inert("Couldn't read its models just now; try /model again.")),
                Some(Listed::Models(list)) if list.is_empty() => items.push(inert(if server.id == "ollama" {
                    "No models yet: pull one that can use tools (ollama pull <model>)."
                } else {
                    "No models yet: get one, then /model again."
                })),
                Some(Listed::Models(list)) => items.extend(list.iter().map(model_item)),
            }
        }
    }
    items
}

/// `/note p3 new words`: the note's id and its new words.
pub(super) fn note_command(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("/note")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let (id, words) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let digits = id.strip_prefix(['p', 's'])?;
    let valid = !digits.is_empty() && digits.len() <= 4 && digits.bytes().all(|b| b.is_ascii_digit());
    (valid && !words.trim().is_empty()).then(|| (id.to_owned(), words.trim().to_owned()))
}

/// What `change` makes of `overlay`, when it's one of the panels.
fn as_panel<R>(overlay: &Shared<TuiApp>, change: impl FnOnce(&mut Panel) -> R) -> Option<R> {
    let mut overlay = overlay.borrow_mut();
    let changed = overlay.downcast_mut::<Panel>().map(change);
    changed
}

impl Overlay<TuiApp> for Panel {
    fn handle(&mut self, app: &TuiApp, event: InputEvent) -> Handled {
        match self {
            Panel::Btw { .. } => app.aside_input(self, event),
            Panel::Job { .. } => app.job_panel_input(self, event),
            Panel::Help { .. } => app.help_input(self, event),
            Panel::Pick { .. } | Panel::Key { .. } | Panel::ChatGpt { .. } => app.panel_key(self, event),
        }
    }
    fn draw(&mut self, app: &TuiApp, screen: &mut Screen, area: Area) -> Option<Cursor> {
        app.draw_panel(screen, self, area.box_top, area.left)
    }
    fn hint(&self, _: &TuiApp) -> String {
        super::draw_panel::panel_hint(self)
    }
    /// A list's own words (a countdown to Kumi's pick, another way to choose) stay over a toast.
    fn hint_stays(&self, _: &TuiApp) -> bool {
        matches!(self, Panel::Pick { picker, .. } if picker.borrow().options.hint.is_some())
    }
    fn closed(&mut self, app: &TuiApp) {
        match self {
            Panel::ChatGpt { abort, .. } => abort.cancel(),
            Panel::Key { secret, .. } => secret.clear(),
            // A side question still being answered stops (unless Kumi, closing, stopped it already).
            Panel::Btw { at, .. } => {
                let aside = app.0.state.borrow().asides.get(*at).cloned();
                if let Some(aside) = aside.filter(|aside| aside.borrow().state == "asking" && !aside.borrow().stopped) {
                    let id = {
                        let mut aside = aside.borrow_mut();
                        aside.stopped = true;
                        aside.id
                    };
                    app.session().tell(op::StopAside { id }.into());
                }
            }
            Panel::Pick { .. } | Panel::Job { .. } | Panel::Help { .. } => {}
        }
    }
    fn busy(&self, app: &TuiApp) -> bool {
        match self {
            Panel::Pick { .. } | Panel::Job { .. } | Panel::Help { .. } => false,
            Panel::Btw { at, .. } => app.0.state.borrow().asides.get(*at).is_some_and(|aside| aside.borrow().state == "asking"),
            Panel::Key { .. } | Panel::ChatGpt { .. } => true,
        }
    }
}
