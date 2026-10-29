use super::super::{
    attach,
    keys::{Modifiers, MouseAction, MouseButton, WheelDirection},
    selection::{Selection, Unit},
    tree::TreeRole,
    width::truncate,
};
use super::panels::AsidePanel;
use super::toast::TOAST_MS;
use super::*;
use crate::text::{library_line, remembered_line};
use base64::Engine;
use kumi_agent::core::commands::{self, Command, Effect, Found, Now, Why};
use std::task::{Context, Poll};

impl TuiApp {
    pub(super) fn on_input(&self, event: InputEvent) {
        if self.0.state.borrow().closing {
            return;
        }
        // Kumi's window came to the front or went behind another: whether to tell the producer what happens next.
        if let InputEvent::Focus { focused } = event {
            self.window_focus(focused);
            return;
        }
        // Someone's there: Kumi doesn't answer its own question for them.
        if matches!(event, InputEvent::Key { .. } | InputEvent::Text { .. } | InputEvent::Paste { .. } | InputEvent::Mouse { .. }) {
            self.stop_answer_countdown();
        }
        if !self.map_shown() && self.selection_input(&event) {
            self.0.scheduler.request();
            return;
        }
        if self.setup_active() {
            // A sign-in under way takes its own keys; its esc goes back to the setup's choices.
            let signing = self.signing_in();
            match event {
                InputEvent::Key { .. } | InputEvent::Text { .. } | InputEvent::Paste { .. } if signing => self.panel_input(event),
                InputEvent::Key { name, mods, .. } => self.setup_key(&name, mods),
                _ => {}
            }
            self.0.scheduler.request();
            return;
        }
        if self.map_shown() {
            self.map_input(event);
            return;
        }
        let panel = self.0.state.borrow().overlays.top();
        let busy_panel = panel.as_ref().is_some_and(|top| top.borrow().busy(self));
        if let Some(voice) = &self.0.voice {
            match &event {
                InputEvent::Key { name, mods, repeat } if name == "t" && mods.ctrl && !mods.alt && !busy_panel => {
                    if panel.is_some() {
                        self.close_panel();
                    }
                    self.0.state.borrow_mut().tree_cursor = None;
                    self.0.tabs.leave();
                    voice.press(*repeat);
                    self.0.scheduler.request();
                    return;
                }
                InputEvent::Release { name, mods } if name == "t" && mods.ctrl && !mods.alt && !busy_panel => {
                    voice.release();
                    self.0.scheduler.request();
                    return;
                }
                _ => {}
            }
        }
        if matches!(event, InputEvent::Release { .. }) {
            return;
        }
        if panel.is_some() && matches!(event, InputEvent::Text { .. } | InputEvent::Paste { .. } | InputEvent::Key { .. }) {
            self.panel_input(event);
            self.maybe_carry_on();
            self.0.scheduler.request();
            return;
        }
        let typed = matches!(event, InputEvent::Text { .. });
        match event {
            InputEvent::Paste { text } if self.can().attachments && attach::pasted_files(&text).is_some() => {
                // Files dragged in arrive as their paths: they go with the next message instead.
                let files: Vec<Attachment> =
                    attach::pasted_files(&text).unwrap_or_default().iter().filter_map(|path| attach::attachment(path)).collect();
                self.0.tabs.leave();
                self.add_attachments(files);
            }
            // In the tabs, a typed character is the row's key: Space opens a change's picture, `u` undoes it.
            InputEvent::Text { text }
                if self.0.tabs.inside()
                    && text.chars().count() == 1
                    && self.0.tabs.key(&if text == " " { "space".to_string() } else { text.to_lowercase() }) => {}
            // The help's typed key, in a block of its own until keys have a registry. `?` arrives as typed text, which
            // key() never sees: on an empty box it opens the help, and anywhere else it types itself.
            // (while looking back through what was sent, `?` is a character to find)
            InputEvent::Text { text }
                if text == "?" && self.0.state.borrow().editor.is_empty() && self.0.state.borrow().search.is_none() =>
            {
                self.open_help()
            }
            // End of the help's typed key.
            InputEvent::Text { text } | InputEvent::Paste { text } => self.type_into_box(&text, typed),
            InputEvent::Key { name, mods, .. } => self.key(&name, mods),
            InputEvent::Mouse { action: MouseAction::Wheel, direction, x, y, .. } => {
                let area = self.0.state.borrow().tabs_area;
                let up = direction == Some(WheelDirection::Up);
                if area.is_some_and(|a| x >= a.x - 2 && x < a.x + a.width + 2 && y >= a.y && y < a.y + a.height) {
                    self.0.tabs.scroll_by(if up { -3 } else { 3 });
                } else if !self.scroll_overlay(if up { -3 } else { 3 }) {
                    self.scroll_by(if up { 3 } else { -3 });
                }
            }
            InputEvent::Mouse { action: MouseAction::Press, button: MouseButton::Left, x, y, .. } => {
                let action =
                    self.0.state.borrow().hits.iter().find(|h| y == h.y && x >= h.x && x < h.x + h.width).map(|h| h.action.clone());
                if let Some(action) = action {
                    action();
                }
            }
            _ => {}
        }
        // The producer may have finished what they were in the middle of: a job's answer can start.
        self.maybe_carry_on();
        self.0.scheduler.request();
    }
    /// Words typed or pasted into the box, at the cursor. A run of typing is one step to undo; a paste is one of its own.
    pub(super) fn type_into_box(&self, text: &str, typed: bool) {
        self.0.tabs.leave();
        let mut state = self.0.state.borrow_mut();
        state.tree_cursor = None;
        let clean = sanitize_text(text, &[]);
        // While looking back through what was sent, they're what to find.
        if state.search.is_some() {
            drop(state);
            self.search_type(&clean);
            return;
        }
        if typed {
            state.editor.type_text(&clean);
        } else {
            // A long paste goes in as one chip.
            state.editor.paste(&clean);
        }
        state.menu_dismissed = false;
    }
    /// Plain dragging selects text even when mouse reporting is needed for scrolling and buttons.
    fn selection_input(&self, event: &InputEvent) -> bool {
        // Anything else the producer does ends the moment just after a copy.
        let copy_key = |name: &str, mods: &Modifiers| name == "escape" || (name == "c" && mods.ctrl && !mods.alt);
        if matches!(event, InputEvent::Key { name, mods, .. } if !copy_key(name, mods))
            || matches!(event, InputEvent::Text { .. } | InputEvent::Paste { .. })
            || matches!(event, InputEvent::Mouse { action: MouseAction::Press | MouseAction::Wheel, .. })
        {
            self.0.state.borrow_mut().copied = None;
        }
        match event {
            InputEvent::Key { name, mods, .. } if name == "c" && mods.ctrl && !mods.alt => {
                let text = self.0.state.borrow().selection.as_ref().filter(|s| s.moved).map(|s| s.text());
                if let Some(text) = text {
                    self.0.state.borrow_mut().selection = None;
                    self.copy_selection(&text);
                    return true;
                }
                // Many terminals' copy key, just after a copy: it copies again, and doesn't stop Kumi, clear the box or
                // quit.
                if let Some(text) = self.just_copied() {
                    self.copy_text(&text);
                    return true;
                }
            }
            InputEvent::Key { name, .. } if name == "escape" => {
                if self.0.state.borrow_mut().selection.take().is_some_and(|s| s.moved) {
                    return true;
                }
                // Just after a copy, Esc takes its toast away. While Kumi works it stops it too, as ever; while it's idle,
                // that's all it does (unpinning, or taking a request back, can wait for the next).
                if self.just_copied().is_some() {
                    self.end_toast();
                    if !self.busy() {
                        return true;
                    }
                }
            }
            InputEvent::Mouse { action: MouseAction::Drag, button: MouseButton::Left, x, y, .. } => {
                if let Some(selection) = self.0.state.borrow_mut().selection.as_mut().filter(|s| s.dragging) {
                    selection.update(*x, *y);
                    return true;
                }
            }
            InputEvent::Mouse { action: MouseAction::Release, button: MouseButton::Left, x, y, .. } => {
                let copy = {
                    let mut state = self.0.state.borrow_mut();
                    let Some(selection) = state.selection.as_mut().filter(|s| s.dragging) else { return false };
                    selection.update(*x, *y);
                    selection.dragging = false;
                    if !selection.moved {
                        state.selection = None;
                        None
                    } else if self.0.options.copy_on_select {
                        // Copied as the button comes up; the view goes on at once.
                        state.selection.take().map(|s| s.text())
                    } else {
                        None
                    }
                };
                // Ctrl-C and Esc just after are the copy's only where "Copied" says so.
                if let Some(text) = copy {
                    if self.copy_selection(&text) && self.toast_shows() {
                        self.0.state.borrow_mut().copied = Some((text, perf_now() + TOAST_MS));
                    }
                }
                return true;
            }
            InputEvent::Mouse { action: MouseAction::Press, button: MouseButton::Left, x, y, .. } => return self.press(*x, *y),
            InputEvent::Mouse { action: MouseAction::Press, button: MouseButton::Right, .. } => {
                let text = self.0.state.borrow().selection.as_ref().filter(|s| s.moved).map(|s| s.text());
                if let Some(text) = text {
                    self.0.state.borrow_mut().selection = None;
                    self.copy_selection(&text);
                    return true;
                }
            }
            InputEvent::Key { .. }
            | InputEvent::Text { .. }
            | InputEvent::Paste { .. }
            | InputEvent::Mouse { action: MouseAction::Wheel, .. } => {
                self.0.state.borrow_mut().selection = None;
            }
            _ => {}
        }
        false
    }
    /// A left press: a double click takes the word under it and a triple click the line (presses on one cell, each
    /// within CLICK_MS of the last); a single one starts a drag, or only puts away the selection on screen. Presses on
    /// what a click works (undo, a tab, the pin) stay clicks.
    fn press(&self, x: i32, y: i32) -> bool {
        let count = {
            let mut state = self.0.state.borrow_mut();
            let now = perf_now();
            let count = match state.clicks {
                Some((at, cx, cy, count)) if now - at <= CLICK_MS && (cx, cy) == (x, y) => count % 3 + 1,
                _ => 1,
            };
            state.clicks = Some((now, x, y, count));
            count
        };
        let hit = self.0.state.borrow().hits.iter().any(|h| y == h.y && x >= h.x && x < h.x + h.width);
        if count > 1 && !hit {
            let unit = if count == 2 { Unit::Word } else { Unit::Line };
            // From the frame the producer sees: the selection's own, still on screen, or the last one drawn.
            let kept = self.0.state.borrow_mut().selection.take();
            let selection = match kept {
                Some(selection) => selection.again(x, y, unit),
                None => self.select(x, y, unit),
            };
            self.0.state.borrow_mut().selection = selection;
            return true;
        }
        // A click on an existing selection only dismisses it; the live view may have changed underneath it.
        if self.0.state.borrow_mut().selection.take().is_some_and(|s| s.moved) {
            return true;
        }
        if hit {
            return false;
        }
        let selection = self.select(x, y, Unit::Cell);
        let started = selection.is_some();
        self.0.state.borrow_mut().selection = selection;
        started
    }
    /// A selection from a press at (x, y) on the frame on screen, with what it says of its cells.
    fn select(&self, x: i32, y: i32, unit: Unit) -> Option<Selection> {
        let screen = self.0.screen.borrow();
        let screen = screen.as_ref().filter(|s| s.width > 0 && s.height > 0)?;
        Selection::new(screen.clone(), self.0.marks.borrow().clone(), x, y, unit)
    }
    /// What letting go of a drag copied, if that was just now: it's taken, so only the next key finds it.
    fn just_copied(&self) -> Option<String> {
        let (text, until) = self.0.state.borrow_mut().copied.take()?;
        (perf_now() < until).then_some(text)
    }
    /// A selection onto the clipboard, said for a moment in the composer's hint row. A selection of blanks copies
    /// nothing, and says so by saying nothing.
    fn copy_selection(&self, text: &str) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        let here = self.copy_text(text);
        self.toast(&copied_words(text, here, self.0.profile.iterm), NoticeTone::Info);
        true
    }
    fn key(&self, name: &str, mods: Modifiers) {
        let Modifiers { ctrl, alt, shift } = mods;
        if let Some(voice) = &self.0.voice {
            if voice.active() {
                if (name == "escape" && self.menu().is_empty()) || (ctrl && name == "c") {
                    voice.cancel();
                    return;
                }
                if name == "enter"
                    && !alt
                    && !shift
                    && self.menu().is_empty()
                    && !is_command(string::trim(&self.0.state.borrow().editor.text()))
                    && voice.enter()
                {
                    return;
                }
            }
        }
        // W5-3's keys, until W3-3's key registry takes them over: composer.history-search (ctrl+r, and the keys while
        // looking), composer.editor (ctrl+g) and composer.stash (ctrl+s).
        if self.search_key(name, mods) {
            return;
        }
        // W4-4: ctrl+o, the conversation in detail (conversation.details in W3-3's registry).
        if ctrl && !alt && name == "o" {
            self.0.tabs.leave();
            self.open_details();
            return;
        }
        if ctrl && !alt && matches!(name, "r" | "g" | "s") {
            self.0.tabs.leave();
            self.0.state.borrow_mut().tree_cursor = None;
            match name {
                "r" => self.search_back(),
                "g" => self.edit_draft(),
                _ => self.stash(),
            }
            return;
        }
        // End of W5-3's keys.
        // ---- W6-2 · the pane: ctrl+\ opens and closes the drawer, or puts the pane away and back (pane.toggle; into the
        // key registry with W3-3) ----
        if ctrl && !alt && name == "\\" {
            self.toggle_pane();
            return;
        }
        // ---- end W6-2 ----
        let menu = self.menu();
        let width = self.input_width();
        if name == "tab" && shift && !ctrl && !alt && menu.is_empty() {
            self.0.state.borrow_mut().tree_cursor = None;
            // (W6-2: the pane shows the tabs the keyboard goes into, the drawer open on them)
            self.open_pane(super::drawer::DrawerTab::Tabs);
            if self.0.tabs.inside() {
                self.0.tabs.next();
            } else {
                self.0.tabs.enter();
            }
            return;
        }
        if self.0.tabs.inside() {
            if matches!(name, "escape" | "tab") {
                self.0.tabs.leave();
                return;
            }
            if !ctrl && !alt && self.0.tabs.key(name) {
                return;
            }
            self.0.tabs.leave();
        }
        let cursor = self.0.state.borrow().tree_cursor;
        if let Some(cursor) = cursor {
            if let Some(rows) = self.tree_shown() {
                if !ctrl && !alt && matches!(name, "up" | "down") {
                    self.0.state.borrow_mut().tree_cursor =
                        Some((cursor as i32 + if name == "up" { -1 } else { 1 }).clamp(0, rows.len() as i32 - 1) as usize);
                    return;
                }
                if !ctrl && !alt && name == "enter" {
                    self.pin(&rows[cursor.min(rows.len() - 1)]);
                    self.0.state.borrow_mut().tree_cursor = None;
                    return;
                }
                if matches!(name, "escape" | "tab") {
                    self.0.state.borrow_mut().tree_cursor = None;
                    return;
                }
            }
            self.0.state.borrow_mut().tree_cursor = None;
        }
        if name == "tab" && menu.is_empty() && !ctrl && !alt && self.busy() {
            let raw = self.composed();
            if !raw.is_empty() && !is_command(string::trim(&raw)) {
                if let Some(history) = &self.0.options.history {
                    history.borrow_mut().add(&raw);
                }
                let mut state = self.0.state.borrow_mut();
                state.recall = None;
                state.editor.clear();
                drop(state);
                self.hold(raw, "after");
                return;
            }
        }
        if name == "tab" && menu.is_empty() && !ctrl && !alt {
            let eligible = {
                let state = self.0.state.borrow();
                // (the chain FOCUS shows: a take's FX chain where one is open, else the track's)
                state
                    .tree
                    .as_ref()
                    .is_some_and(|t| state.focus.as_ref().and_then(|f| f.chain_ref.as_ref().or(f.track_ref.as_ref())) == Some(&t.track_ref))
            };
            if eligible {
                // (W6-2: the drawer open on FOCUS, where the keyboard goes)
                self.open_pane(super::drawer::DrawerTab::Focus);
                self.0.state.borrow_mut().tree_cursor = Some(0);
                let shown = self.tree_shown();
                self.0.state.borrow_mut().tree_cursor = shown.map(|rows| rows.iter().position(|r| r.role == TreeRole::Focus).unwrap_or(0));
            } else {
                self.0.state.borrow_mut().tree_cursor = None;
            }
            return;
        }
        if ctrl && name == "c" {
            if self.busy() {
                self.cancel();
            } else if !self.0.state.borrow().editor.is_empty() {
                self.erase_draft();
            } else {
                drop(self.finish(0, None));
            }
            return;
        }
        if ctrl && name == "b" && self.can().jobs {
            self.background_now();
            return;
        }
        if ctrl && name == "v" && self.can().attachments {
            self.paste_picture();
            return;
        }
        if ctrl && name == "d" {
            if self.0.state.borrow().editor.is_empty() && !self.busy() {
                drop(self.finish(0, None));
            } else {
                self.0.state.borrow_mut().editor.delete();
            }
            return;
        }
        if name == "escape" {
            let checking = self.0.state.borrow().slots_check.is_some();
            if !menu.is_empty() {
                self.0.state.borrow_mut().menu_dismissed = true;
            } else if checking {
                // A /slots fetch or check stops first.
                let check = self.0.state.borrow_mut().slots_check.take();
                if let Some(id) = check {
                    self.session().tell(op::StopSlots { id }.into());
                }
            } else if self.busy() {
                self.cancel();
            } else if self.0.state.borrow().pinned.is_some() {
                self.unpin();
            }
            return;
        }
        if !menu.is_empty() && matches!(name, "up" | "down") {
            let mut state = self.0.state.borrow_mut();
            state.menu_index = (state.menu_index + if name == "up" { menu.len() - 1 } else { 1 }) % menu.len();
            state.menu_chosen = Some(state.editor.text().to_string());
            return;
        }
        if !menu.is_empty() && name == "tab" {
            let mut state = self.0.state.borrow_mut();
            let index = state.menu_index;
            state.editor.set(menu[index].command.name);
            return;
        }
        if name == "enter" && !alt && !shift {
            if !menu.is_empty() {
                let mut state = self.0.state.borrow_mut();
                let found = menu[state.menu_index];
                let typed = state.editor.text().to_string();
                let chosen = state.menu_chosen.as_deref() == Some(typed.as_str());
                state.editor.set(found.command.name);
                // A command found only by the words of what it does is none the producer named: Enter puts its name in the
                // box, as tab does, unless they chose it with ↑↓.
                if !found.named() && !chosen {
                    return;
                }
            }
            self.submit_task();
            return;
        }
        let mut state = self.0.state.borrow_mut();
        if name == "enter" || (ctrl && name == "j") {
            state.editor.insert("\n");
            return;
        }
        if name == "backspace" && state.editor.is_empty() && state.attachments.pop().is_some() {
            return;
        }
        if name == "backspace" {
            if alt || ctrl {
                state.editor.delete_word_left();
            } else {
                state.editor.backspace();
            }
            return;
        }
        if name == "delete" {
            state.editor.delete();
            return;
        }
        if name == "left" {
            if alt || ctrl {
                state.editor.word_left();
            } else {
                state.editor.left();
            }
            return;
        }
        if name == "right" {
            if alt || ctrl {
                state.editor.word_right();
            } else {
                state.editor.right();
            }
            return;
        }
        if alt && name == "b" {
            state.editor.word_left();
            return;
        }
        if alt && name == "f" {
            state.editor.word_right();
            return;
        }
        if alt && name == "up" {
            if let Some(at) = state.held.iter().rposition(|h| !h.taken) {
                let item = state.held.remove(at);
                let text = if state.editor.is_empty() { item.text } else { format!("{}\n{}", item.text, state.editor.text()) };
                state.editor.set(&text);
                state.recall = None;
                return;
            }
        }
        if name == "up" {
            let moved = state.editor.vertical(width, -1);
            drop(state);
            if !moved {
                self.recall_older();
            }
            return;
        }
        if name == "down" {
            let moved = state.editor.vertical(width, 1);
            drop(state);
            if !moved {
                self.recall_newer();
            }
            return;
        }
        if ctrl && name == "home" {
            let State { view, transcript, .. } = &mut *state;
            view.to_start(transcript, perf_now());
            return;
        }
        if ctrl && name == "end" {
            state.view.follow();
            return;
        }
        if name == "home" || (ctrl && name == "a") {
            state.editor.home();
            return;
        }
        if name == "end" || (ctrl && name == "e") {
            state.editor.end();
            return;
        }
        // W5-2's keys, until W3-3's key registry takes them over: composer.undo (ctrl+-, which legacy terminals send
        // as ctrl+_), composer.yank (ctrl+y) and alt+y, which swaps what ctrl+y put in for the cut before it.
        if ctrl && matches!(name, "-" | "_") {
            state.editor.undo();
            return;
        }
        if ctrl && name == "y" {
            state.editor.yank();
            return;
        }
        if alt && name == "y" {
            state.editor.yank_earlier();
            return;
        }
        // End of W5-2's keys.
        if ctrl && name == "k" {
            state.editor.kill_to_end();
            return;
        }
        if ctrl && name == "u" {
            state.editor.kill_to_start();
            return;
        }
        if ctrl && name == "w" {
            state.editor.delete_word_left();
            return;
        }
        if name == "pageup" || name == "pagedown" {
            let page = if name == "pageup" { state.page } else { -state.page };
            let State { view, transcript, .. } = &mut *state;
            view.scroll_by(transcript, page, perf_now());
            return;
        }
        if ctrl && name == "l" {
            self.0.renderer.borrow_mut().invalidate();
        }
    }
    pub(super) fn menu(&self) -> Vec<Found> {
        let text = {
            let state = self.0.state.borrow();
            let text = state.editor.text().to_string();
            if state.menu_dismissed
                || state.search.is_some()
                || !text.starts_with('/')
                || text.chars().any(|c| string::trim(&c.to_string()).is_empty())
            {
                return vec![];
            }
            text
        };
        let has = self.has();
        let recent = self.recent_commands();
        let recent: Vec<&str> = recent.iter().map(String::as_str).collect();
        let matches = commands::matching(&text, &recent, |command| has.offers(command));
        let mut state = self.0.state.borrow_mut();
        if state.menu_index >= matches.len() {
            state.menu_index = 0;
        }
        // A choice made with ↑↓ holds for the words it was made for.
        if state.menu_chosen.as_deref().is_some_and(|chosen| chosen != text) {
            state.menu_chosen = None;
        }
        matches
    }
    /// The commands sent lately, latest first, each once: the `/` menu lists them first.
    fn recent_commands(&self) -> Vec<String> {
        let Some(history) = &self.0.options.history else { return vec![] };
        let mut recent: Vec<String> = Vec::new();
        for line in history.borrow().sent().rev().take(RECENT_LOOK) {
            let line = string::trim(line);
            if let Some(name) = line.split_whitespace().next().filter(|_| is_command(line)) {
                if !recent.iter().any(|seen| seen == name) {
                    recent.push(name.to_string());
                }
            }
        }
        recent
    }
    pub(super) fn submit_task(&self) {
        let app = self.clone();
        let mut future = async move {
            if let Err(error) = app.submit().await {
                app.error(&error);
            }
        }
        .boxed_local();
        if let Poll::Pending = future.as_mut().poll(&mut Context::from_waker(futures::task::noop_waker_ref())) {
            tokio::task::spawn_local(future);
        }
    }
    /// The composer's words as they leave it, cleaned again as one text: what each keystroke or paste added was cleaned
    /// alone, and a run of variation selectors split between two reads is only one run here.
    fn composed(&self) -> String {
        sanitize_text(&self.0.state.borrow().editor.expanded(), &[])
    }
    pub(super) async fn submit(&self) -> Result<(), RuntimeError> {
        let raw = self.composed();
        // The conversation shows a long paste as its chip and the start of its first line; the model gets all of it.
        let shown = self.0.state.borrow().editor.shown();
        let command = string::trim(&raw);
        if command.is_empty() {
            return Ok(());
        }
        let session = self.session();
        if let Some(history) = &self.0.options.history {
            history.borrow_mut().add(&raw);
        }
        self.0.state.borrow_mut().recall = None;
        let goal_running = {
            let state = self.0.state.borrow();
            state.goal.is_some() || state.objective.is_some()
        };
        let now = Now { busy: self.busy(), goal_running, can: self.can(), fronts: self.has().fronts };
        match commands::run(command, &now) {
            Effect::Quit => {
                self.clear_editor();
                self.finish(0, None).await;
                return Ok(());
            }
            Effect::Help => {
                self.clear_editor();
                self.open_help();
                return Ok(());
            }
            Effect::Changelog => {
                self.clear_editor();
                let since = self.0.state.borrow().news_since.clone();
                match crate::whats_new::changelog(kumi_types::version::KUMI_VERSION, since.as_deref()) {
                    Some(news) => {
                        let footer = Some(format!("Every release: {}", crate::whats_new::history_link()));
                        self.0.state.borrow_mut().transcript.add(Entry::News { title: news.title, items: news.items, footer });
                        self.0.scheduler.request();
                    }
                    None => self.notice("Kumi's changelog has no notes for this version.", NoticeTone::Info),
                }
                return Ok(());
            }
            Effect::Jobs => {
                self.clear_editor();
                self.open_jobs();
                return Ok(());
            }
            Effect::Voice => {
                self.clear_editor();
                self.open_voice(None);
                return Ok(());
            }
            Effect::Fast => {
                let Some(models) = self.models() else { return Ok(()) };
                self.clear_editor();
                let current = models.current();
                // The setting, not whether this model uses it: a saved "on" can always be turned off.
                let on = !models.fast_enabled();
                let name = current.name.or(current.model).unwrap_or_else(|| "This model".into());
                match models.set_fast(on).await {
                    Ok(Some(tier)) => self.notice(
                        &format!(
                            "{} is on{}. /fast again turns it off.",
                            tier.name,
                            tier.description.map(|d| format!(": {d}")).unwrap_or_default()
                        ),
                        NoticeTone::Info,
                    ),
                    Ok(None) if on => self.notice(&format!("{name} has no faster tier to turn on."), NoticeTone::Info),
                    Ok(None) => self.notice("Back to the standard tier.", NoticeTone::Info),
                    Err(error) => self.error(&error),
                }
                return Ok(());
            }
            Effect::Model | Effect::Effort | Effect::Login | Effect::Logout => {
                self.clear_editor();
                let result = match command {
                    "/model" => self.open_models().await,
                    "/effort" => self.open_effort().await,
                    "/login" => self.open_login().await,
                    _ => self.open_logout().await,
                };
                if let Err(e) = result {
                    self.panel_failed(&e);
                }
                return Ok(());
            }
            Effect::Recipes => {
                self.clear_editor();
                if let Err(e) = self.open_recipes().await {
                    self.panel_failed(&e);
                }
            }
            Effect::Memory => {
                self.clear_editor();
                if let Err(e) = self.open_memory().await {
                    self.panel_failed(&e);
                }
            }
            Effect::Refused(Why::NotKept) => {
                self.clear_editor();
                self.notice(&super::not_kept(), NoticeTone::Info);
                return Ok(());
            }
            Effect::Resume { word } => {
                self.clear_editor();
                let found = if word.is_empty() { None } else { Some(session.op(op::FindConversation { word: word.to_string() }).await?) };
                let result = match found {
                    None => self.open_conversations(None).await,
                    Some(ConversationMatch::One(row)) if row.current => {
                        self.notice("That's this conversation.", NoticeTone::Info);
                        Ok(())
                    }
                    Some(ConversationMatch::One(row)) => {
                        self.go_back_to(&row).await;
                        Ok(())
                    }
                    Some(ConversationMatch::Several(_)) => self.open_conversations(Some(word)).await,
                    Some(ConversationMatch::Gone) => {
                        self.notice("That conversation isn't kept any more.", NoticeTone::Info);
                        Ok(())
                    }
                    Some(ConversationMatch::None) => {
                        self.notice(&format!("No conversation {}; /resume lists them.", self.clean(&word, 60)), NoticeTone::Info);
                        Ok(())
                    }
                };
                if let Err(e) = result {
                    self.panel_failed(&e);
                }
                return Ok(());
            }
            Effect::Rename { words } => {
                if words.is_empty() {
                    let title = session.op(op::ConversationNow {}).await?.map(|now| now.title).unwrap_or_default();
                    self.0.state.borrow_mut().editor.set(&format!("/rename {title}"));
                    self.0.scheduler.request();
                    return Ok(());
                }
                self.clear_editor();
                let (first, rest) = words.split_once(' ').map(|(a, b)| (a, string::trim(b))).unwrap_or((words.as_str(), ""));
                let kept = !rest.is_empty()
                    && matches!(session.op(op::FindConversation { word: first.into() }).await?, ConversationMatch::One(row) if row.id == first);
                let (id, name) = if kept { (Some(first), rest) } else { (None, words.as_str()) };
                if session.op(op::RenameConversation { id: id.map(Into::into), name: Some(name.into()) }).await? {
                    self.notice(&format!("Named “{}”.", self.clean(name, 60)), NoticeTone::Info);
                } else {
                    self.notice("That conversation isn't kept any more.", NoticeTone::Info);
                }
                return Ok(());
            }
            Effect::Fork { name } => {
                self.clear_editor();
                if self.busy() {
                    self.notice("Kumi is still working: fork once it's done, or press esc to stop it.", NoticeTone::Info);
                    return Ok(());
                }
                if session.op(op::ForkConversation { name: (!name.is_empty()).then(|| name.clone()) }).await?.is_none() {
                    self.notice("Nothing to fork yet: ask something first.", NoticeTone::Info);
                }
                return Ok(());
            }
            Effect::Recipe => {
                let (name, with) = match panels::recipe_command(command) {
                    Ok(parsed) => parsed,
                    Err(how) => {
                        self.notice(&how, NoticeTone::Info);
                        return Ok(());
                    }
                };
                let recipes = session.op(op::Recipes {}).await?;
                let lower = name.to_lowercase();
                let Some(recipe) =
                    recipes.iter().find(|r| r.name == name).or_else(|| recipes.iter().find(|r| r.name.to_lowercase() == lower))
                else {
                    self.notice(&format!("Kumi keeps no recipe called “{name}”; /recipes lists them."), NoticeTone::Info);
                    return Ok(());
                };
                if let Some(problem) = panels::recipe_blanks_problem(recipe, &with) {
                    self.notice(&problem, NoticeTone::Warn);
                    return Ok(());
                }
                // The line stays in the box while Kumi works, to run once it's done.
                if self.busy() {
                    self.notice("Kumi is still working: press enter again once it's done, or esc to stop it.", NoticeTone::Info);
                    return Ok(());
                }
                self.clear_editor();
                self.run_recipe_now(recipe, with).await;
                return Ok(());
            }
            Effect::Note => {
                let Some((id, words)) = panels::note_command(command) else {
                    self.notice("Give the note's id and its new words: /note p3 Prefers short reverbs on drums", NoticeTone::Info);
                    return Ok(());
                };
                match session.op(op::ChangeNote { id: id.clone(), change: NoteChange::Text(words) }).await {
                    Ok(Some(_)) => {
                        self.clear_editor();
                        self.notice(&format!("Changed note {id}."), NoticeTone::Info);
                    }
                    Ok(None) => self.notice(&format!("There's no note {id}; /memory lists them."), NoticeTone::Info),
                    Err(error) => self.notice(&error.message(), NoticeTone::Warn),
                }
                return Ok(());
            }
            Effect::Stop => {
                self.clear_editor();
                if self.0.state.borrow().connection != ConnectionState::Connected {
                    self.notice(
                        &format!("{} isn't connected, so there's nothing for Kumi to stop.", crate::daw::words().app),
                        NoticeTone::Info,
                    );
                    return Ok(());
                }
                if self.busy() {
                    let _ = session.op(op::Cancel {}).await;
                }
                if !session.op(op::StopDaw {}).await? {
                    self.notice(
                        &format!("Kumi couldn't stop {0} just now; press space in {0} to stop it.", crate::daw::words().app),
                        NoticeTone::Warn,
                    );
                }
                return Ok(());
            }
            Effect::Daw => {
                let Some(switch) = self.0.options.switch_daw.clone() else { return Ok(()) };
                self.clear_editor();
                match crate::daw::daw_command(&command["/daw".len()..]) {
                    crate::daw::DawCommand::Say(words) => self.notice(&words, NoticeTone::Info),
                    crate::daw::DawCommand::Switch(_, _) if self.busy() => {
                        self.notice("Kumi is working; /daw once it's done (esc stops it).", NoticeTone::Info);
                    }
                    crate::daw::DawCommand::Switch(daw, words) => {
                        self.notice(&words, NoticeTone::Info);
                        switch(daw);
                        self.reopen().await;
                    }
                }
                return Ok(());
            }
            Effect::Update => {
                self.clear_editor();
                if let Err(e) = self.open_update().await {
                    self.panel_failed(&e);
                }
                return Ok(());
            }
            Effect::Slots => {
                self.clear_editor();
                let words = command["/slots".len()..].to_string();
                // One swap at a time: a model's fetch can take minutes.
                if self.0.state.borrow().slots_check.is_some()
                    && kumi_listening::slots::parse(&words) != Ok(kumi_listening::slots::Asked::Show)
                {
                    self.notice("Kumi is still trying a model for /slots: wait for it, or Esc stops it.", NoticeTone::Info);
                    return Ok(());
                }
                // Its own stop, Esc: a model's download takes as long as it takes (a listening model's check has its own
                // minute and a half). What's under way comes as notices (`slots-progress` events).
                let id = {
                    let mut state = self.0.state.borrow_mut();
                    let id = state.next_slots;
                    state.next_slots += 1;
                    state.slots_check.get_or_insert(id);
                    id
                };
                let said = session.op(op::Slots { id, words }).await;
                let ended = self.0.state.borrow().slots_check == Some(id);
                if ended {
                    self.0.state.borrow_mut().slots_check = None;
                }
                match said {
                    Ok(kumi_listening::slots::Said::Slots { lines, footer }) => {
                        self.0.state.borrow_mut().transcript.add(Entry::News {
                            title: "Model slots".into(),
                            items: lines,
                            footer: Some(footer),
                        });
                        self.0.scheduler.request();
                    }
                    Ok(kumi_listening::slots::Said::Done(text)) => self.notice(&text, NoticeTone::Info),
                    Ok(kumi_listening::slots::Said::Refused(text)) => self.notice(&text, NoticeTone::Warn),
                    Err(error) => self.notice(&error.to_string(), NoticeTone::Warn),
                }
                return Ok(());
            }
            Effect::Music => {
                self.clear_editor();
                self.music_command(&command["/music".len()..]).await;
                return Ok(());
            }
            Effect::Map => {
                self.clear_editor();
                self.map_command(&command["/map".len()..]).await;
                return Ok(());
            }
            Effect::Palette => {
                self.clear_editor();
                self.palette_command(&command["/palette".len()..]).await;
                return Ok(());
            }
            Effect::Status => {
                self.clear_editor();
                let status = session.status();
                let library = session.library().or_else(|| self.0.state.borrow().library.clone());
                let library = library_line(library.as_ref());
                let state = if status.state == TurnState::Idle { "Ready".into() } else { enum_name(&status.state) };
                let turns = if let Some(max) = status.max_turns.filter(|n| *n != 0) {
                    format!("{} of {max} turns", status.turns)
                } else {
                    format!("{} {}", status.turns, if status.turns == 1 { "turn" } else { "turns" })
                };
                self.notice(
                    &format!(
                        "{state} · {} · {} · {turns}{}{}{}",
                        match self.0.state.borrow().focus.as_ref().and_then(|focus| focus.busy.clone()) {
                            Some(why) => format!("{} is busy: {why}", crate::daw::words().app),
                            None => format!("{} {}", crate::daw::words().app, enum_name(&status.connection)),
                        },
                        self.model_label().unwrap_or_else(|| "no model".into()),
                        status.observation.as_ref().filter(|s| !s.is_empty()).map(|s| format!(" · {s}")).unwrap_or_default(),
                        library.map(|l| format!(" · {l}")).unwrap_or_default(),
                        self.tokens_used()
                    ),
                    NoticeTone::Info,
                );
                // Where Kumi's lines reach a producer working in the DAW (Live's say so in its own settings): its status bar, or
                // none to write to.
                let profile = crate::daw::profile();
                if !matches!(profile.id, Some(kumi_types::daw::DawId::Ableton) | None) {
                    let app = profile.words.app;
                    self.notice(
                        &if profile.caps.status_line {
                            format!("While you work in {app}, Kumi's lines show in its status bar too.")
                        } else {
                            format!("{app} has no status bar Kumi can write to: while you work there, Kumi tells you by notification.")
                        },
                        NoticeTone::Info,
                    );
                }
                if let Some(line) = remembered_line(session.op(op::Remembered {}).await?) {
                    self.notice(&line, NoticeTone::Info);
                }
                if let Some(now) = session.op(op::ConversationNow {}).await? {
                    self.notice(&conversation_line(&now, |text| self.clean(text, 60)), NoticeTone::Info);
                }
                return Ok(());
            }
            Effect::Btw => {
                self.clear_editor();
                let question = string::trim(&command[4..]);
                if !question.is_empty() {
                    self.ask(question);
                } else {
                    let len = self.0.state.borrow().asides.len();
                    if len > 0 {
                        self.open_overlay(AsidePanel { at: len - 1, scroll: 0 });
                    } else {
                        self.notice("Ask on the side with /btw and your question: Kumi answers from the conversation so far, without stopping what it's doing.",NoticeTone::Info);
                    }
                }
                self.0.scheduler.request();
                return Ok(());
            }
            Effect::Message if now.busy => {
                let when = {
                    let mut state = self.0.state.borrow_mut();
                    state.editor.clear();
                    if state.current.is_none() && !state.pending_turn {
                        state.activity = "getting ready".into();
                    }
                    if state.current.is_some() || state.pending_turn {
                        "now"
                    } else {
                        "after"
                    }
                };
                self.hold(raw, when);
                return Ok(());
            }
            effect @ (Effect::PauseGoal | Effect::PauseLoop) => {
                self.clear_editor();
                if self.busy() {
                    self.cancel();
                } else {
                    self.notice(
                        if effect == Effect::PauseLoop { "There's no loop running to pause." } else { "There's no goal running to pause." },
                        NoticeTone::Info,
                    );
                }
            }
            Effect::StopGoal => {
                self.clear_editor();
                if !session.op(op::StopGoal {}).await? {
                    self.notice("There's no goal to stop.", NoticeTone::Info);
                }
                return Ok(());
            }
            Effect::StopLoop => {
                self.clear_editor();
                if !(self.can().stop_loop && session.op(op::StopLoop {}).await?) {
                    if self.busy() {
                        self.cancel();
                    } else {
                        self.notice("There's no loop to stop.", NoticeTone::Info);
                    }
                }
                return Ok(());
            }
            Effect::ShowGoal => {
                self.clear_editor();
                self.0.tabs.show("goal");
                // (W6-2: asked for, the drawer opens on it)
                self.open_pane(super::drawer::DrawerTab::Tabs);
                self.0.scheduler.request();
                return Ok(());
            }
            Effect::Refused(Why::Busy { verb }) => {
                self.notice(&format!("Kumi is still working: {} once it's done, or press esc to stop it first.", verb), NoticeTone::Info);
                return Ok(());
            }
            Effect::Goal => {
                self.clear_editor();
                let text = string::trim(&command[5..]);
                let clean = string::trim(&self.clean(&raw, usize::MAX)).to_string();
                {
                    let mut state = self.0.state.borrow_mut();
                    if !text.is_empty() {
                        state.transcript.add(Entry::User { text: clean });
                    }
                    state.activity = if !text.is_empty() { "setting up the goal" } else { "picking the goal up" }.into();
                    state.pending_turn = true;
                }
                self.0.tabs.show("goal");
                self.0.scheduler.request();
                if let Err(error) = session.op(op::Goal { text: (!text.is_empty()).then(|| text.to_string()) }).await {
                    self.0.state.borrow_mut().pending_turn = false;
                    if !self.0.state.borrow().closing {
                        self.error(&error);
                    }
                }
                self.0.scheduler.request();
                return Ok(());
            }
            Effect::Loop => {
                self.clear_editor();
                self.send(&raw).await;
                return Ok(());
            }
            Effect::Undo => {
                self.clear_editor();
                self.undo(None).await;
                return Ok(());
            }
            Effect::Copy => {
                self.clear_editor();
                self.copy_last_answer();
                return Ok(());
            }
            Effect::Export { words } => {
                self.clear_editor();
                self.export_as(&words).await;
                return Ok(());
            }
            Effect::Share => {
                self.clear_editor();
                self.share().await;
                return Ok(());
            }
            Effect::Willington => {
                let Some((willington, on)) = self.willington() else { return Ok(()) };
                self.clear_editor();
                match (willington.set)(!on).await {
                    Ok(said) => {
                        self.0.state.borrow_mut().willington_off = on;
                        self.notice(said, NoticeTone::Info);
                        // The bridge picks the switch up within a second; the model, from the next message.
                        session.op(op::Reconfigure {}).await?;
                    }
                    Err(error) => self.notice(
                        &format!("Kumi couldn't turn Willington's bindings {}: {error}", if on { "off" } else { "on" }),
                        NoticeTone::Warn,
                    ),
                }
                self.0.scheduler.request();
                return Ok(());
            }
            Effect::Refresh => {
                self.ready_for_more();
                let result = {
                    self.0.state.borrow_mut().activity = "reading your Set".into();
                    session.op(op::Refresh {}).await
                };
                self.settled(result);
            }
            Effect::New { name } => {
                self.ready_for_more();
                let result = {
                    {
                        let mut state = self.0.state.borrow_mut();
                        state.activity = "starting fresh".into();
                        if !state.transcript.is_empty() {
                            state.transcript.add(Entry::Divider { text: "New conversation. Kumi won't use what's above".into() });
                        }
                        state.current = None;
                        state.watching = false;
                        // (W6-1: a fresh conversation fills the model's context anew)
                        state.last_call = None;
                    }
                    let started = session.op(op::NewConversation {}).await;
                    // Its name, kept with it from its first save.
                    if started.is_ok() && !name.is_empty() {
                        if self.can().conversations {
                            session.op(op::RenameConversation { id: None, name: Some(name.clone()) }).await?;
                        } else {
                            self.notice(&super::not_kept(), NoticeTone::Info);
                        }
                    }
                    started
                };
                self.settled(result);
            }
            Effect::Reconnect => {
                self.ready_for_more();
                let result = {
                    self.0.state.borrow_mut().activity = format!("connecting to {}", crate::daw::words().app);
                    let result = session.op(op::Reconnect {}).await;
                    // REAPER's and Renoise's integrations retire their rows as they close, each in its DAW's words.
                    if result.is_ok() && crate::daw::current().id() == kumi_types::daw::DawId::Ableton {
                        for change in self.0.state.borrow_mut().records.changes_mut() {
                            if matches!(change.state, ChangeState::Applied | ChangeState::Unsure) {
                                change.state = ChangeState::Expired;
                                change.note =
                                    Some("Kumi reconnected to Live since, so it can't undo this; Live's own undo still can.".into());
                            }
                        }
                    }
                    result
                };
                self.settled(result);
            }
            Effect::Unknown { name } => {
                self.ready_for_more();
                let result = {
                    self.notice(&format!("There's no {} command. Type / to see them.", name), NoticeTone::Info);
                    Ok(())
                };
                self.settled(result);
            }
            Effect::Message => {
                self.ready_for_more();
                let text = string::trim(&self.clean(&shown, usize::MAX)).to_string();
                {
                    let mut state = self.0.state.borrow_mut();
                    state.transcript.add(Entry::User { text });
                    state.last_sent = Some(raw.clone());
                }
                if self.instant(&raw).await {
                    return Ok(());
                }
                self.send(&raw).await;
                return Ok(());
            }
        }
        Ok(())
    }
    /// The box cleared and the conversation back at its latest, as a line runs.
    fn ready_for_more(&self) {
        self.clear_editor();
        self.0.state.borrow_mut().view.follow();
    }
    /// What a line asked of the session, settled: what went wrong said (unless Kumi is closing), and the screen drawn.
    fn settled(&self, result: Result<(), RuntimeError>) {
        if let Err(error) = result {
            self.0.state.borrow_mut().pending_turn = false;
            if !self.0.state.borrow().closing {
                self.error(&error);
            }
        }
        self.0.scheduler.request();
    }
    fn clear_editor(&self) {
        self.0.state.borrow_mut().editor.clear();
    }
    /// ctrl+c on words not sent: they're kept among the history's entries while Kumi runs, whole and never written to its
    /// file, so ↑ brings them back, and the box empties as a step ctrl+- takes back. An earlier message ↑ is showing,
    /// unchanged, is in the history already.
    fn erase_draft(&self) {
        let (text, shown) = {
            let state = self.0.state.borrow();
            (state.editor.expanded(), state.recall.as_ref().map(|(index, _)| *index))
        };
        if let Some(history) = &self.0.options.history {
            let unchanged = shown.is_some_and(|index| history.borrow().entries().get(index) == Some(&text));
            if !unchanged {
                history.borrow_mut().keep(&text);
            }
        }
        self.0.state.borrow_mut().editor.erase();
    }
    /// /willington, with whether the bindings are on, while the bridge in Live carries Willington.
    pub(super) fn willington(&self) -> Option<(kumi_daw::provider::Toggle, bool)> {
        let willington = self.0.options.willington.clone()?;
        let on = (willington.on)()?;
        Some((willington, on))
    }
    /// Files that go with the next message; the same file twice is added once.
    pub(super) fn add_attachments(&self, files: Vec<Attachment>) {
        let mut state = self.0.state.borrow_mut();
        for file in files {
            if !state.attachments.iter().any(|a| a.path == file.path) {
                state.attachments.push(file);
            }
        }
        drop(state);
        self.0.scheduler.request();
    }
    /// ctrl+v: a picture on the clipboard, such as a screenshot, goes with the next message.
    fn paste_picture(&self) {
        // One read at a time: PowerShell takes a couple of seconds to start.
        if std::mem::replace(&mut self.0.state.borrow_mut().pasting, true) {
            return;
        }
        self.notice("Reading the clipboard…", NoticeTone::Info);
        self.task(|app| async move {
            let env = attach::lossy_env();
            let folder = std::path::PathBuf::from(crate::config::kumi_dir(&env)).join("attachments");
            let read = attach::clipboard_picture(&folder).await;
            app.0.state.borrow_mut().pasting = false;
            match read {
                Ok(Some(path)) => app.add_attachments(attach::attachment(&path).into_iter().collect()),
                Ok(None) => app.notice("There's no picture on the clipboard. Drag a file in, or paste its path.", NoticeTone::Info),
                Err(why) => app.notice(&format!("Kumi couldn't read the clipboard: {why}"), NoticeTone::Warn),
            }
            Ok(())
        });
    }
    /// A message that's a fixed phrase ("tempo 124", "mute drums") changes Live at once, with no model call; its
    /// answer says what changed, marked instant, and the change is in HISTORY with its undo. Anything else, or a phrase
    /// Kumi can't run as asked (a track it can't tell for sure), goes to the model as written.
    async fn instant(&self, raw: &str) -> bool {
        if !self.0.options.instant
            || !self.can().instant
            || !self.0.state.borrow().attachments.is_empty()
            || kumi_agent::core::phrases::parse(raw).is_none()
        {
            return false;
        }
        let started = perf_now();
        {
            let mut state = self.0.state.borrow_mut();
            state.instant_running = true;
            state.activity = "changing".into();
        }
        self.0.scheduler.request();
        let outcome = self.session().op(op::Instant { text: raw.into() }).await;
        self.0.state.borrow_mut().instant_running = false;
        let Ok(Some(outcome)) = outcome else { return false };
        let text = string::trim(&self.clean(&outcome.text, 2000)).to_string();
        let mut state = self.0.state.borrow_mut();
        state.transcript.add(Entry::Assistant {
            text: if outcome.is_error { text } else { format!("{text} · instant") },
            steps: vec![],
            status: if outcome.is_error { AnswerStatus::Failed } else { AnswerStatus::Done },
            elapsed_ms: Some(number::round(perf_now() - started)),
            started_at: Some(started),
            thinking: None,
            footer: None,
        });
        state.view.follow();
        drop(state);
        self.0.scheduler.request();
        true
    }
    pub(super) async fn send(&self, raw: &str) -> bool {
        let (pinned, attachments) = {
            let mut state = self.0.state.borrow_mut();
            if state.closing {
                return false;
            }
            state.activity = "thinking".into();
            state.pending_turn = true;
            (state.pinned.as_ref().map(|p| p.pin.clone()), std::mem::take(&mut state.attachments))
        };
        self.0.scheduler.request();
        let sent = self.session().op(op::Submit { input: raw.into(), pinned, attachments: attachments.clone(), references: vec![] }).await;
        if let Err(error) = sent {
            // Files that didn't go stay, to change or send again.
            let mut state = self.0.state.borrow_mut();
            let added = std::mem::replace(&mut state.attachments, attachments);
            state.attachments.extend(added);
            drop(state);
            self.0.state.borrow_mut().pending_turn = false;
            if !self.0.state.borrow().closing {
                self.error(&error);
            }
            self.0.scheduler.request();
            return false;
        }
        self.0.scheduler.request();
        true
    }
    fn ask(&self, question: &str) {
        let id = {
            let mut state = self.0.state.borrow_mut();
            state.next_aside += 1;
            state.next_aside - 1
        };
        let aside = Rc::new(RefCell::new(Aside {
            id,
            question: self.clean(question, usize::MAX),
            answer: String::new(),
            stream: StreamingText::new(&self.0.state.borrow().secrets),
            state: "asking",
            stopped: false,
        }));
        let at = {
            let mut state = self.0.state.borrow_mut();
            state.asides.push(aside.clone());
            if state.asides.len() > 20 {
                state.asides.remove(0);
            }
            state.asides.len() - 1
        };
        self.open_overlay(AsidePanel { at, scroll: 0 });
        let question = question.to_string();
        self.task(move |app| async move {
            // Its words come as `aside` events while it's answered.
            let result = app.session().op(op::Aside { id, question }).await;
            match result {
                Ok(answer) => {
                    let mut aside = aside.borrow_mut();
                    let tail = aside.stream.finish();
                    aside.answer.push_str(&tail);
                    if string::trim(&aside.answer).is_empty() {
                        aside.answer = app.clean(&answer, usize::MAX);
                    }
                    aside.state = "done";
                }
                Err(error) => {
                    let mut aside = aside.borrow_mut();
                    aside.state = "failed";
                    if !aside.stopped {
                        aside.answer = safe_error_message(Some(&error.message()), &app.0.state.borrow().secrets);
                    }
                }
            }
            if !app.0.state.borrow().closing {
                app.0.scheduler.request();
            }
            Ok(())
        });
    }
    /// Text onto the clipboard: through the terminal (OSC 52, which reaches the producer's clipboard over SSH too), and
    /// with this computer's own tool where Kumi runs in front of the producer, for terminals that don't take OSC 52.
    /// Says whether this computer's tool took it; the terminal never says.
    pub(super) fn copy_text(&self, text: &str) -> bool {
        self.0.tty.write(&format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(text)));
        self.0.options.copy.as_ref().is_some_and(|copy| copy(text))
    }
    fn copy_last_answer(&self) {
        let answer = self.0.state.borrow().transcript.entries.iter().rev().find_map(|e| {
            if let Entry::Assistant { text, .. } = &*e.borrow() {
                (!string::trim(text).is_empty()).then(|| text.clone())
            } else {
                None
            }
        });
        let Some(answer) = answer else {
            self.notice("There's no answer to copy yet.", NoticeTone::Info);
            return;
        };
        let text = string::trim(&self.clean(&answer, usize::MAX)).to_string();
        if text.len() > 64 * 1024 {
            self.notice("That answer is too long to copy this way; hold Shift and drag to select it.", NoticeTone::Info);
            return;
        }
        self.copy_text(&text);
        self.notice(
            "Copied Kumi's last answer. If it didn't arrive, your terminal may not allow it: hold Shift and drag to select instead.",
            NoticeTone::Info,
        );
    }
    pub(super) async fn undo(&self, id: Option<&str>) {
        if self.0.state.borrow().closing || self.0.state.borrow().undoing {
            return;
        }
        if self.busy() {
            self.notice("Kumi is still working. Press esc to stop it first, then undo.", NoticeTone::Info);
            return;
        }
        if id.is_none() && !self.0.state.borrow().records.changes().iter().any(|c| c.state == ChangeState::Applied) {
            self.notice("There's nothing of Kumi's to undo.", NoticeTone::Info);
            return;
        }
        {
            let mut state = self.0.state.borrow_mut();
            state.undoing = true;
            state.activity = "undoing".into();
        }
        self.0.scheduler.request();
        match self.session().op(op::Undo { id: id.map(Into::into) }).await {
            Ok(Some(change)) => {
                if change.state == ChangeState::Undone {
                    self.notice(&format!("Undid: {}", change.title), NoticeTone::Info);
                } else {
                    self.notice(
                        string::trim(&format!("Kept: {}. {}", change.title, change.note.as_deref().unwrap_or(""))),
                        NoticeTone::Warn,
                    );
                }
            }
            Err(error) => {
                if !self.0.state.borrow().closing {
                    self.error(&error);
                }
            }
            _ => {}
        }
        self.0.state.borrow_mut().undoing = false;
        self.0.scheduler.request();
    }
    /// /takeback: the producer's last request taken back, or their last `words` of them. Kumi undoes their changes newest
    /// first and puts the earliest one's words back in the box.
    pub(super) async fn take_back(&self, words: &str) {
        if self.0.state.borrow().closing || self.0.state.borrow().undoing {
            return;
        }
        if !self.can().take_back {
            self.notice("Kumi can't take requests back here: this conversation isn't kept.", NoticeTone::Info);
            return;
        }
        if self.busy() {
            self.notice("Kumi is still working. Press esc to stop it first, then take it back.", NoticeTone::Info);
            return;
        }
        let Some(count) = (if words.is_empty() { Some(1) } else { words.parse::<usize>().ok().filter(|n| *n > 0) }) else {
            self.notice("/takeback takes back your last request, and /takeback 3 your last three.", NoticeTone::Info);
            return;
        };
        {
            let mut state = self.0.state.borrow_mut();
            state.undoing = true;
            state.activity = "taking it back".into();
        }
        self.0.scheduler.request();
        let taken = match self.session().op(op::TakeBackPlan {}).await {
            // Newest first: the earliest of the last `count`, or the first of all when there are fewer.
            Ok(requests) => match requests.get(count.min(requests.len()).saturating_sub(1)) {
                Some(earliest) => self.session().op(op::TakeBack { to: earliest.at }).await.map(|taken| Some((taken, requests))),
                None => Ok(None),
            },
            Err(error) => Err(error),
        };
        match taken {
            Ok(Some((taken, requests))) => {
                // What the requests that went changed that Kumi couldn't undo, as the plan had them: left as they are.
                let left: Vec<ChangeRecord> = requests
                    .iter()
                    .take(taken.requests as usize)
                    .flat_map(|asked| &asked.changes)
                    .filter(|change| !matches!(change.state, ChangeState::Applied | ChangeState::Undone | ChangeState::Heard))
                    .cloned()
                    .collect();
                self.taken_back(taken, left);
            }
            Ok(None) => self.notice("There's no request of yours to take back yet.", NoticeTone::Info),
            Err(error) => {
                if !self.0.state.borrow().closing {
                    self.error(&error);
                }
            }
        }
        self.0.state.borrow_mut().undoing = false;
        self.0.scheduler.request();
    }
    /// What taking requests back did, in words, with the earliest one's words back in the box.
    fn taken_back(&self, taken: TakenBack, left: Vec<ChangeRecord>) {
        let many = |n: usize, one: &str, more: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {more}") };
        let undid = (!taken.undone.is_empty()).then(|| many(taken.undone.len(), "change", "changes"));
        let mut said = vec![];
        match (taken.requests, undid) {
            (0, None) => {}
            (0, Some(undid)) => said.push(format!("Undid {undid}.")),
            (n, undid) => said.push(format!(
                "Took back {}{}.",
                if n == 1 { "your last request".to_string() } else { format!("your last {n} requests") },
                undid.map(|undid| format!(" and undid {undid}")).unwrap_or_default()
            )),
        }
        if let Some(failed) = &taken.failed {
            said.push(
                string::trim(&format!("Kumi couldn't undo {} and stopped there. {}", failed.title, failed.note.as_deref().unwrap_or("")))
                    .to_string(),
            );
        }
        if !left.is_empty() {
            let why = |change: &ChangeRecord| match change.state {
                ChangeState::Expired => "from an earlier session",
                ChangeState::Unsure => "never confirmed",
                _ => "Kumi can't undo it",
            };
            let named: Vec<String> = left.iter().map(|change| format!("{} ({})", change.title, why(change))).collect();
            said.push(format!("Left as {}: {}.", if left.len() == 1 { "it is" } else { "they are" }, named.join("; ")));
        }
        if let Some(words) = taken.words.filter(|words| !words.is_empty()) {
            self.0.state.borrow_mut().editor.set(&words);
            said.push("Its words are back in the box to change and send again.".into());
        }
        let tone = if taken.failed.is_some() { NoticeTone::Warn } else { NoticeTone::Info };
        self.notice(&said.join(" "), tone);
    }
    pub(super) fn scroll_by(&self, lines: i32) {
        let mut state = self.0.state.borrow_mut();
        let State { view, transcript, .. } = &mut *state;
        view.scroll_by(transcript, lines, perf_now());
    }
    pub(super) fn layout_for(columns: i32) -> (i32, i32) {
        let pane = if columns >= super::drawer::PANE_COLUMNS { ((columns as f64 * 0.34).floor() as i32).clamp(36, 46) } else { 0 };
        (pane, columns - pane)
    }
    /// The side pane's width and the conversation's at this many columns: none for the pane once put away (ctrl+\).
    pub(super) fn layout(&self, columns: i32) -> (i32, i32) {
        if self.0.state.borrow().pane_away {
            (0, columns)
        } else {
            Self::layout_for(columns)
        }
    }
    pub(super) fn input_width(&self) -> i32 {
        (self.layout(self.0.tty.size().columns).1 - 6).max(1)
    }
}
/// How soon a press on the same cell counts with the one before: a double or a triple click.
const CLICK_MS: f64 = 400.;
/// What a toast says of a copy: the words, when they're on one line, else how many lines. Where only the terminal
/// took them (over SSH, or with no clipboard tool here), they arrive only if it allows it, so it says how to use the
/// terminal's own selection instead (Option-drag in iTerm2).
fn copied_words(text: &str, here: bool, iterm: bool) -> String {
    let lines = text.lines().count();
    if here {
        return if lines > 1 { format!("Copied {lines} lines") } else { format!("Copied “{}”", truncate(text.trim(), 40)) };
    }
    let what = if lines > 1 { format!(" {lines} lines") } else { String::new() };
    format!("Copied{what} if the terminal allows it · or {}-drag", if iterm { "option" } else { "shift" })
}

/// How many of the lines sent last the `/` menu looks through for the commands used lately.
const RECENT_LOOK: usize = 200;

/// What a command does, as the menu and the help say it: in the DAW's words (Live's as written, for Live).
pub(super) fn about(command: &Command) -> String {
    command.about(crate::daw::words(), crate::daw::current().id() == kumi_types::daw::DawId::Ableton)
}
fn enum_name(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}
