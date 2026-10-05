use super::super::{
    picker::{NoteTone, PickerOptions},
    width::graphemes,
};
use super::*;
use crate::{
    models::{ChatGptSignIn, LocalStatus, ProviderStatus},
    voice::VoiceChange,
};
use base64::Engine;
use kumi_runtime::{
    core::errors::FailureKind,
    integrations::ableton::project::since,
    providers::{
        models::{ApiKeyCheck, ModelInfo},
        provider_info, Effort, SignIn,
    },
};
use std::collections::HashMap;

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
fn item(label: impl Into<String>, value: impl Into<String>, detail: impl Into<String>) -> PickerItem {
    let detail = detail.into();
    PickerItem { detail: (!detail.is_empty()).then_some(detail), ..PickerItem::new(label, value) }
}
fn inert(label: impl Into<String>) -> PickerItem {
    PickerItem { label: label.into(), inert: true, ..Default::default() }
}
fn noted(mut item: PickerItem, note: impl Into<String>, tone: NoteTone) -> PickerItem {
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
        let draft = state.recall.as_ref().map(|r| r.1.clone()).unwrap_or_else(|| state.editor.text());
        state.recall = Some((index, draft));
        state.editor.set(&entries[index]);
        state.menu_dismissed = true;
    }
    pub(super) fn recall_newer(&self) {
        let entries = self.0.options.history.as_ref().map(|h| h.borrow().entries().to_vec()).unwrap_or_default();
        let mut state = self.0.state.borrow_mut();
        let Some((index, draft)) = state.recall.clone() else {
            return;
        };
        let index = index + 1;
        if index >= entries.len() {
            state.editor.set(&draft);
            state.recall = None;
            return;
        }
        state.recall = Some((index, draft));
        state.editor.set(&entries[index]);
        state.menu_dismissed = true;
    }
    pub(super) async fn open_memory(&self) -> Result<(), RuntimeError> {
        let c = &self.0.options.controller;
        let (memory, techniques, recipes, lessons, taste) =
            futures::try_join!(c.memory(), c.techniques(), c.recipes(), c.lessons(), c.taste())?;
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
        items.push(PickerItem::heading(format!("About {}", memory.set_name.as_deref().unwrap_or("this Set"))));
        if !memory.saved {
            items.push(inert("Kept once the Set is saved"));
        } else if memory.memory.set.is_empty() {
            items.push(inert("Nothing yet"));
        } else {
            items.extend(notes(&memory.memory.set));
        }
        if c.has_techniques() {
            items.push(PickerItem::heading("Techniques"));
            if techniques.is_empty() {
                items.push(inert("None yet: what worked in things Kumi built that you liked"));
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
        if c.has_recipes() {
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
        if c.has_lessons() {
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
        if c.has_taste() {
            items.push(PickerItem::heading("From your Sets"));
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
                        "Learning your Sets…"
                    } else {
                        "Nothing yet: how you work, from your own Live Sets"
                    },
                ));
            } else {
                items.extend(taste.iter().map(|l| PickerItem::new(self.clean(&l.line.replace('\n', " "), 160), format!("taste:{}", l.id))));
            }
        }
        self.pick(
            Picker::with_options(
                "What Kumi remembers · notes, techniques and recipes",
                items,
                PickerOptions { filterable: true, hint: None },
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
                    if kind == "note" && app.0.options.controller.has_change_note() {
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
                                let c = &app.0.options.controller;
                                let gone = match kind.as_str() {
                                    "technique" => c.forget_technique(&id).await?,
                                    "lesson" => c.forget_lesson(&id).await?,
                                    "taste" => c.forget_taste(&id).await?,
                                    _ => c.forget(&id).await?.is_some(),
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
    pub(super) async fn open_conversations(&self) -> Result<(), RuntimeError> {
        let kept = self.0.options.controller.conversations().await?;
        let now = now_ms_f64();
        let items = if kept.is_empty() {
            vec![inert("None kept yet: a conversation is kept once you've asked something")]
        } else {
            kept.iter()
                .map(|row| {
                    let label = self.clean(&row.first.replace('\n', " "), 120);
                    noted(
                        PickerItem::new(if label.is_empty() { "(nothing asked yet)".into() } else { label }, &row.id),
                        format!(
                            "{} · {} {}",
                            if row.current { "this one".into() } else { since(row.saved_at as f64, now) },
                            row.turns,
                            if row.turns == 1 { "request" } else { "requests" }
                        ),
                        NoteTone::Faint,
                    )
                })
                .collect()
        };
        let title = format!("Conversations about {}", self.0.state.borrow().set_name.as_deref().unwrap_or("this Set"));
        self.pick(Picker::with_options(title, items, PickerOptions { filterable: true, hint: None }), move |app, item| {
            let row = kept.iter().find(|r| Some(&r.id) == item.value.as_ref()).cloned();
            async move {
                app.close_panel();
                let Some(row) = row.filter(|r| !r.current) else {
                    return Ok(());
                };
                if app.busy() {
                    app.notice("Kumi is still working. Press esc to stop it first.", NoticeTone::Info);
                    return Ok(());
                }
                app.0.state.borrow_mut().activity = "going back to it".into();
                match app.0.options.controller.resume_conversation(&row.id).await {
                    Ok(false) => app.notice("That conversation isn't kept any more.", NoticeTone::Info),
                    Err(error) => app.error(&error),
                    _ => {}
                }
                Ok(())
            }
        });
        Ok(())
    }
    pub(super) async fn open_recipes(&self) -> Result<(), RuntimeError> {
        let recipes = self.0.options.controller.recipes().await?;
        let now = now_ms_f64();
        let items = if recipes.is_empty() {
            vec![inert("None yet: ask Kumi to save a way of working, or say “watch me” and do it in Live")]
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
                PickerOptions { filterable: true, hint: None },
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
                    let c = &app.0.options.controller;
                    match answer.value.as_deref() {
                        Some("change") => {
                            app.0.state.borrow_mut().editor.set(&format!("/note {} {}", note.id, note.text));
                            app.0.scheduler.request();
                        }
                        Some("pin") => match c.change_note(&note.id, NoteChange::Pinned(!note.pinned)).await? {
                            Some(changed) if changed.pinned => {
                                app.notice("Pinned: Kumi keeps this note even when its memory is full.", NoticeTone::Info)
                            }
                            Some(_) => app.notice("Unpinned.", NoticeTone::Info),
                            None => app.notice("That note was already gone.", NoticeTone::Info),
                        },
                        Some("forget") => {
                            if c.forget(&note.id).await?.is_none() {
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
                        if !app.0.options.controller.forget_recipe(&recipe.name).await? {
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
                        let pinned = app.0.state.borrow().pinned.as_ref().map(|p| p.pin.clone());
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
        if self.0.options.controller.has_run_recipe() {
            let outcome = self.0.options.controller.run_recipe(&recipe.name, with).await.unwrap_or_else(|error| RecipeOutcome {
                text: safe_error_message(Some(&error.message()), &self.0.state.borrow().secrets),
                is_error: true,
            });
            self.notice(
                &if outcome.is_error { format!("The recipe stopped: {}", outcome.text) } else { outcome.text },
                if outcome.is_error { NoticeTone::Warn } else { NoticeTone::Info },
            );
        }
    }
    fn sign_in(&self, provider: ProviderId, then: Option<Action>) {
        let Some(models) = self.0.options.models.clone() else {
            return;
        };
        if provider_info(provider).sign_in == SignIn::ApiKey {
            self.0.state.borrow_mut().panel =
                Some(Rc::new(RefCell::new(Panel::Key { provider, secret: String::new(), checking: false, status: None, then })));
            self.0.scheduler.request();
            return;
        }
        let abort = Signal::new();
        let panel = Rc::new(RefCell::new(Panel::ChatGpt { url: None, abort: abort.clone(), then }));
        self.0.state.borrow_mut().panel = Some(panel.clone());
        self.0.scheduler.request();
        let weak = Rc::downgrade(&self.0);
        let shown = panel.clone();
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
        self.task(move |app| async move {
            let result = models.sign_in_chatgpt(ChatGptSignIn::Browser { signal: abort.clone(), on_url }).await;
            match result {
                Ok(()) => {
                    if app.same_panel(&panel) {
                        app.0.state.borrow_mut().panel = None;
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
                    if app.same_panel(&panel) {
                        app.0.state.borrow_mut().panel = None;
                    }
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
            app.0.scheduler.request();
            Ok(())
        });
    }
    async fn submit_key(&self, panel: PanelRef) {
        let (provider, key, then) = {
            let mut p = panel.borrow_mut();
            let Panel::Key { provider, secret, checking, status, then } = &mut *p else {
                return;
            };
            *checking = true;
            *status = Some((format!("Checking the key with {}…", provider_info(*provider).name), NoticeTone::Info));
            (*provider, secret.clone(), then.clone())
        };
        self.0.scheduler.request();
        let name = provider_info(provider).name;
        let verdict = match self.0.options.models.as_ref().unwrap().save_key(provider, &key, None).await {
            Ok(v) => v,
            Err(error) => {
                let mut secrets = self.0.state.borrow().secrets.clone();
                secrets.push(key);
                if let Panel::Key { checking, status, .. } = &mut *panel.borrow_mut() {
                    *checking = false;
                    *status = Some((safe_error_message(Some(&error.message()), &secrets), NoticeTone::Warn));
                }
                self.0.scheduler.request();
                return;
            }
        };
        if verdict == ApiKeyCheck::Refused {
            if let Panel::Key { checking, secret, status, .. } = &mut *panel.borrow_mut() {
                *checking = false;
                secret.clear();
                *status = Some((format!("{name} didn't accept that key. Paste it again, or esc to leave it."), NoticeTone::Warn));
            }
            self.0.scheduler.request();
            return;
        }
        {
            let mut state = self.0.state.borrow_mut();
            state.secrets.push(key.clone());
            state.stream.add_secret(key);
        }
        if let Panel::Key { secret, .. } = &mut *panel.borrow_mut() {
            secret.clear();
        }
        if self.same_panel(&panel) {
            self.0.state.borrow_mut().panel = None;
        }
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
    pub(super) fn offer_fix(&self, kind: &str, provider: Option<&str>) {
        let Some(models) = &self.0.options.models else {
            return;
        };
        if self.0.state.borrow().panel.is_some() {
            return;
        }
        if kind == "config" && models.current().model.is_none() {
            self.task(|app| async move {
                if let Err(error) = app.open_models().await {
                    app.panel_failed(&error);
                }
                Ok(())
            });
            return;
        }
        if kind == "network" && provider.is_some_and(|p| ProviderId::parse(p).is_none()) {
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
        if matches!(kind, "model" | "config") {
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
        let Some(provider) = provider.and_then(ProviderId::parse).filter(|_| kind == "auth") else {
            return;
        };
        let info = provider_info(provider);
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
    pub(super) async fn check_model(&self) -> Result<(), RuntimeError> {
        let Some(models) = &self.0.options.models else {
            return Ok(());
        };
        if self.0.state.borrow().panel.is_some() || self.0.state.borrow().closing {
            return Ok(());
        }
        let current = models.current();
        if current.model.is_none() {
            let chosen = models.choose_default().await?;
            if self.0.state.borrow().closing {
                return Ok(());
            }
            if let Some(chosen) = chosen {
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
                return Ok(());
            }
            self.notice("Sign in to a provider to talk to its models: ChatGPT with your plan, or others with an API key. Or open Ollama or LM Studio to use models on this computer.",NoticeTone::Info);
            if self.0.state.borrow().panel.is_none() {
                self.open_models().await?;
            }
            return Ok(());
        }
        let status = models.providers().await?.into_iter().find(|p| Some(p.id.as_str()) == current.provider.as_deref());
        if let Some(status) = &status {
            if !status.signed_in {
                self.offer_fix("auth", Some(status.id.as_str()));
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
            }
        }
        Ok(())
    }
    fn action<F, Fut>(&self, f: F) -> Action
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
    fn pick<F, Fut>(&self, picker: Picker, f: F) -> Rc<RefCell<Picker>>
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
        self.0.state.borrow_mut().panel = Some(Rc::new(RefCell::new(Panel::Pick { picker: picker.clone(), choose })));
        self.0.scheduler.request();
        picker
    }
    fn same_panel(&self, panel: &PanelRef) -> bool {
        self.0.state.borrow().panel.as_ref().is_some_and(|p| Rc::ptr_eq(p, panel))
    }
    fn same_picker(&self, picker: &Rc<RefCell<Picker>>) -> bool {
        self.0
            .state
            .borrow()
            .panel
            .as_ref()
            .is_some_and(|p| matches!(&*p.borrow(),Panel::Pick{picker:active,..}if Rc::ptr_eq(active,picker)))
    }
    pub(super) fn close_panel(&self) {
        let panel = self.0.state.borrow_mut().panel.take();
        if let Some(panel) = panel {
            match &mut *panel.borrow_mut() {
                Panel::ChatGpt { abort, .. } => abort.cancel(),
                Panel::Key { secret, .. } => secret.clear(),
                Panel::Btw { at, .. } => {
                    if let Some(aside) = self.0.state.borrow().asides.get(*at) {
                        if aside.borrow().state == "asking" {
                            aside.borrow().abort.cancel();
                        }
                    }
                }
                _ => {}
            }
        }
        self.0.scheduler.request();
    }
    pub(super) fn panel_failed(&self, error: &RuntimeError) {
        self.close_panel();
        if !self.0.state.borrow().closing {
            self.error(error);
        }
    }
    pub(super) fn panel_input(&self, event: InputEvent) {
        let Some(panel) = self.0.state.borrow().panel.clone() else {
            return;
        };
        if matches!(&*panel.borrow(), Panel::Btw { .. }) {
            self.aside_input(&panel, event);
            return;
        }
        if let InputEvent::Key { name, mods, .. } = &event {
            if name == "escape" || (mods.ctrl && name == "c") {
                self.close_panel();
                return;
            }
            let mut p = panel.borrow_mut();
            match &mut *p {
                Panel::Pick { picker, choose } => match name.as_str() {
                    "up" => picker.borrow_mut().r#move(-1),
                    "down" | "tab" => picker.borrow_mut().r#move(1),
                    "backspace" => picker.borrow_mut().erase(),
                    "enter" => {
                        let item = picker.borrow().selected().cloned();
                        if let Some(item) = item {
                            let choose = choose.clone();
                            self.task(move |app| async move {
                                if let Err(error) = choose(item).await {
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
                        drop(p);
                        let panel = panel.clone();
                        self.task(move |app| async move {
                            app.submit_key(panel).await;
                            Ok(())
                        });
                    }
                }
                _ => {}
            }
            return;
        }
        let text = match &event {
            InputEvent::Text { text } | InputEvent::Paste { text } => text,
            _ => return,
        };
        let mut p = panel.borrow_mut();
        match &mut *p {
            Panel::ChatGpt { url: Some(url), .. } if text.to_lowercase() == "c" => {
                let url = url.clone();
                drop(p);
                self.0.tty.write(&format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(url)));
                self.notice("Copied the sign-in link.", NoticeTone::Info);
            }
            Panel::Pick { picker, .. } => {
                if picker.borrow().filter.is_empty() && text.starts_with('/') {
                    drop(p);
                    self.close_panel();
                    self.on_input(event);
                    return;
                }
                picker.borrow_mut().r#type(text);
            }
            Panel::Key { secret, checking, status, .. } if !*checking => {
                let clean: String =
                    text.chars().filter(|c| !string::trim(&c.to_string()).is_empty() && (*c as u32) > 31 && *c != '\x7f').collect();
                *secret = head(&(secret.clone() + &clean), 4096);
                *status = None;
            }
            _ => {}
        }
    }
    fn aside_input(&self, panel: &PanelRef, event: InputEvent) {
        if let InputEvent::Key { name, mods, .. } = &event {
            if matches!(name.as_str(), "escape" | "enter") || (mods.ctrl && name == "c") {
                self.close_panel();
                return;
            }
            if let Panel::Btw { at, scroll } = &mut *panel.borrow_mut() {
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
            }
            return;
        }
        if let InputEvent::Text { text } = event {
            if text == " " {
                self.close_panel();
                return;
            }
            if text.to_lowercase() == "c" {
                let at = if let Panel::Btw { at, .. } = &*panel.borrow() {
                    *at
                } else {
                    return;
                };
                let answer = self.0.state.borrow().asides.get(at).map(|a| string::trim(&a.borrow().answer).to_string()).unwrap_or_default();
                if !answer.is_empty() {
                    self.0.tty.write(&format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(answer)));
                    self.notice("Copied the side answer.", NoticeTone::Info);
                }
            }
        }
    }
    pub(super) fn model_label(&self) -> Option<String> {
        let current = self.0.options.models.as_ref()?.current();
        let Some(model) = current.model else {
            return Some("no model chosen".into());
        };
        let name = current.name.unwrap_or_else(|| model.split_once('/').map(|(_, m)| m).unwrap_or(&model).into());
        Some(current.effort.map(|e| format!("{name} · {}", e.as_str())).unwrap_or(name))
    }
    pub(super) fn tokens_used(&self) -> String {
        let Some(provider) = self.0.options.models.as_ref().and_then(|m| m.current().provider).as_deref().and_then(ProviderId::parse)
        else {
            return String::new();
        };
        let state = self.0.state.borrow();
        if provider_info(provider).sign_in != SignIn::ApiKey || state.used.answers == 0 {
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
        let Some(models) = self.0.options.models.clone() else {
            return Ok(());
        };
        let picker = self.pick(
            Picker::with_options("Choose a model", vec![inert("Reading your sign-ins…")], PickerOptions { filterable: true, hint: None }),
            |app, item| async move { app.choose_model_item(item).await },
        );
        let m = models.clone();
        let servers = tokio::task::spawn_local(async move { m.local().await });
        let statuses = Rc::new(models.providers().await?);
        let current = models.current().model;
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
                        picker.borrow_mut().set_items(model_items(
                            &statuses,
                            &lists.borrow(),
                            local.borrow().as_deref(),
                            current.as_deref(),
                        ));
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
    async fn choose_model_item(&self, item: PickerItem) -> Result<(), RuntimeError> {
        let models = self.0.options.models.as_ref().unwrap();
        let value = item.value.as_deref().unwrap_or("");
        if let Some(provider) = value.strip_prefix("signin:").and_then(ProviderId::parse) {
            self.sign_in(provider, Some(self.action(|app| async move { app.open_models().await })));
            return Ok(());
        }
        let note = match models.choose(value).await {
            Ok(note) => note,
            Err(error) => {
                if let Some(provider) =
                    error.kumi().filter(|e| e.kind == FailureKind::Auth).and_then(|e| e.provider.as_deref()).and_then(ProviderId::parse)
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
                "Kumi talks to {} from your next message{effort}.{}{}",
                current.name.as_deref().unwrap_or(value),
                if current.pinned { " KUMI_MODEL is set, so this lasts until Kumi closes." } else { "" },
                note.map(|n| format!(" {n}")).unwrap_or_default()
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
                    Effort::Low => "Fastest; lighter thinking",
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
        self.pick(Picker::new("Sign in to", items), |app, item| async move {
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
            let info = provider_info(provider);
            let shared = if info.credential == "opencode" { " (OpenCode Zen and Go share it)" } else { "" };
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
                    let still = if models.providers().await?.iter().find(|p| p.id == provider).and_then(|p| p.via.as_deref())
                        == Some("environment")
                    {
                        format!(" {} is still set, so Kumi uses that key now.", info.key_env.unwrap_or("undefined"))
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
                    app.finish(0, None).await;
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
            focus.and_then(|f| f.clip.as_ref()),
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
                    let panel = app.0.state.borrow().panel.clone();
                    if let Some(panel) = panel {
                        if let Panel::Pick { picker, .. } = &*panel.borrow() {
                            picker.borrow_mut().set_items(app.voice_items());
                        }
                    }
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
        if self.0.state.borrow().panel.is_some() || self.0.state.borrow().closing {
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
fn model_items(
    statuses: &[ProviderStatus],
    lists: &HashMap<String, Listed>,
    local: Option<&[LocalStatus]>,
    current: Option<&str>,
) -> Vec<PickerItem> {
    let model_item = |m: &ModelInfo| {
        let i = item(&m.name, &m.id, m.description.clone().unwrap_or_default());
        if Some(m.id.as_str()) == current {
            noted(i, "current", NoteTone::Accent)
        } else {
            i
        }
    };
    let mut items = vec![];
    for p in statuses {
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
