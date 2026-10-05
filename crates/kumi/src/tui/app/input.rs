use super::super::{
    keys::{Modifiers, MouseAction, MouseButton, WheelDirection},
    tree::TreeRole,
};
use super::*;
use crate::text::library_line;
use base64::Engine;
use std::task::{Context, Poll};

impl TuiApp {
    pub(super) fn on_input(&self, event: InputEvent) {
        if self.0.state.borrow().closing {
            return;
        }
        let panel = self.0.state.borrow().panel.clone();
        let busy_panel = panel.as_ref().is_some_and(|p| match &*p.borrow() {
            Panel::Pick { .. } => false,
            Panel::Btw { at, .. } => self.0.state.borrow().asides.get(*at).is_some_and(|a| a.borrow().state == "asking"),
            _ => true,
        });
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
            self.0.scheduler.request();
            return;
        }
        match event {
            InputEvent::Text { text } | InputEvent::Paste { text } => {
                self.0.tabs.leave();
                let mut state = self.0.state.borrow_mut();
                state.tree_cursor = None;
                state.editor.insert(&sanitize_text(&text, &[]));
                state.menu_dismissed = false;
            }
            InputEvent::Key { name, mods, .. } => self.key(&name, mods),
            InputEvent::Mouse { action: MouseAction::Wheel, direction, x, y, .. } => {
                let area = self.0.state.borrow().tabs_area;
                let up = direction == Some(WheelDirection::Up);
                if area.is_some_and(|a| x >= a.x - 2 && x < a.x + a.width + 2 && y >= a.y && y < a.y + a.height) {
                    self.0.tabs.scroll_by(if up { -3 } else { 3 });
                } else {
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
        self.0.scheduler.request();
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
        let menu = self.menu();
        let width = self.input_width();
        if name == "tab" && shift && !ctrl && !alt && menu.is_empty() {
            self.0.state.borrow_mut().tree_cursor = None;
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
            let raw = self.0.state.borrow().editor.text();
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
                state.tree.as_ref().is_some_and(|t| state.focus.as_ref().and_then(|f| f.track_ref.as_ref()) == Some(&t.track_ref))
            };
            if eligible {
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
                self.0.state.borrow_mut().editor.clear();
            } else {
                drop(self.finish(0, None));
            }
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
            if !menu.is_empty() {
                self.0.state.borrow_mut().menu_dismissed = true;
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
            return;
        }
        if !menu.is_empty() && name == "tab" {
            let mut state = self.0.state.borrow_mut();
            let index = state.menu_index;
            state.editor.set(menu[index].name);
            return;
        }
        if name == "enter" && !alt && !shift {
            if !menu.is_empty() {
                let mut state = self.0.state.borrow_mut();
                let index = state.menu_index;
                state.editor.set(menu[index].name);
            }
            self.submit_task();
            return;
        }
        let mut state = self.0.state.borrow_mut();
        if name == "enter" || (ctrl && name == "j") {
            state.editor.insert("\n");
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
            state.scroll = i32::MAX;
            return;
        }
        if ctrl && name == "end" {
            state.scroll = 0;
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
        if name == "pageup" {
            state.scroll = state.scroll.saturating_add(state.page).max(0);
            return;
        }
        if name == "pagedown" {
            state.scroll = state.scroll.saturating_sub(state.page).max(0);
            return;
        }
        if ctrl && name == "l" {
            self.0.renderer.borrow_mut().invalidate();
        }
    }
    pub(super) fn menu(&self) -> Vec<&'static Command> {
        let mut state = self.0.state.borrow_mut();
        let text = state.editor.text();
        if state.menu_dismissed || !text.starts_with('/') || text.chars().any(|c| string::trim(&c.to_string()).is_empty()) {
            return vec![];
        }
        let c = &self.0.options.controller;
        let matches: Vec<_> = COMMANDS
            .iter()
            .filter(|cmd| {
                cmd.name.starts_with(&text)
                    && match cmd.name {
                        "/model" | "/effort" | "/login" | "/logout" => self.0.options.models.is_some(),
                        "/memory" => c.has_memory(),
                        "/recipes" => c.has_recipes(),
                        "/conversations" => c.has_conversations(),
                        "/reconnect" => c.has_reconnect(),
                        "/stop" => c.has_stop_live(),
                        "/update" => self.0.options.updates.is_some(),
                        "/btw" => c.has_aside(),
                        "/voice" => self.0.voice.is_some(),
                        _ => true,
                    }
            })
            .collect();
        if state.menu_index >= matches.len() {
            state.menu_index = 0;
        }
        matches
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
    pub(super) async fn submit(&self) -> Result<(), RuntimeError> {
        let raw = self.0.state.borrow().editor.text();
        let command = string::trim(&raw);
        if command.is_empty() {
            return Ok(());
        }
        let controller = &self.0.options.controller;
        if let Some(history) = &self.0.options.history {
            history.borrow_mut().add(&raw);
        }
        self.0.state.borrow_mut().recall = None;
        if command == "/quit" {
            self.clear_editor();
            self.finish(0, None).await;
            return Ok(());
        }
        if command == "/help" {
            self.clear_editor();
            self.notice(HELP, NoticeTone::Info);
            return Ok(());
        }
        if command == "/voice" && self.0.voice.is_some() {
            self.clear_editor();
            self.open_voice(None);
            return Ok(());
        }
        if self.0.options.models.is_some() && matches!(command, "/model" | "/effort" | "/login" | "/logout") {
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
        for (cmd, available) in [
            ("/recipes", controller.has_recipes()),
            ("/conversations", controller.has_conversations()),
            ("/memory", controller.has_memory()),
        ] {
            if command == cmd && available {
                self.clear_editor();
                let result = match cmd {
                    "/recipes" => self.open_recipes().await,
                    "/conversations" => self.open_conversations().await,
                    _ => self.open_memory().await,
                };
                if let Err(e) = result {
                    self.panel_failed(&e);
                }
                return Ok(());
            }
        }
        if command == "/stop" && controller.has_stop_live() {
            self.clear_editor();
            if self.0.state.borrow().connection != ConnectionState::Connected {
                self.notice("Live isn't connected, so there's nothing for Kumi to stop.", NoticeTone::Info);
                return Ok(());
            }
            if self.busy() {
                let _ = controller.cancel().await;
            }
            if !controller.stop_live().await? {
                self.notice("Kumi couldn't stop Live just now; press space in Live to stop it.", NoticeTone::Warn);
            }
            return Ok(());
        }
        if command == "/update" && self.0.options.updates.is_some() {
            self.clear_editor();
            if let Err(e) = self.open_update().await {
                self.panel_failed(&e);
            }
            return Ok(());
        }
        if command == "/status" {
            self.clear_editor();
            let status = controller.status();
            let library = controller.library().or_else(|| self.0.state.borrow().library.clone());
            let library = library_line(library.as_ref());
            let state = if status.state == TurnState::Idle { "Ready".into() } else { enum_name(&status.state) };
            let turns = if let Some(max) = status.max_turns.filter(|n| *n != 0) {
                format!("{} of {max} turns", status.turns)
            } else {
                format!("{} {}", status.turns, if status.turns == 1 { "turn" } else { "turns" })
            };
            self.notice(
                &format!(
                    "{state} · Live {} · {} · {turns}{}{}{}",
                    enum_name(&status.connection),
                    self.model_label().unwrap_or_else(|| "no model".into()),
                    status.observation.as_ref().filter(|s| !s.is_empty()).map(|s| format!(" · {s}")).unwrap_or_default(),
                    library.map(|l| format!(" · {l}")).unwrap_or_default(),
                    self.tokens_used()
                ),
                NoticeTone::Info,
            );
            return Ok(());
        }
        if (command == "/btw" || command.starts_with("/btw ")) && controller.has_aside() {
            self.clear_editor();
            let question = string::trim(&command[4..]);
            if !question.is_empty() {
                self.ask(question);
            } else {
                let len = self.0.state.borrow().asides.len();
                if len > 0 {
                    self.0.state.borrow_mut().panel = Some(Rc::new(RefCell::new(Panel::Btw { at: len - 1, scroll: 0 })));
                } else {
                    self.notice("Ask on the side with /btw and your question: Kumi answers from the conversation so far, without stopping what it's doing.",NoticeTone::Info);
                }
            }
            self.0.scheduler.request();
            return Ok(());
        }
        if self.busy() && !is_command(command) {
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
        if matches!(command, "/goal stop" | "/goal end") && controller.has_stop_goal() {
            self.clear_editor();
            if !controller.stop_goal().await? {
                self.notice("There's no goal to stop.", NoticeTone::Info);
            }
            return Ok(());
        }
        if command == "/goal" && self.busy() && self.0.state.borrow().goal.is_some() {
            self.clear_editor();
            self.0.tabs.show("goal");
            self.0.scheduler.request();
            return Ok(());
        }
        if self.busy() {
            self.notice(
                &format!(
                    "Kumi is still working: {} once it's done, or press esc to stop it first.",
                    command.split_whitespace().next().unwrap_or("")
                ),
                NoticeTone::Info,
            );
            return Ok(());
        }
        if (command == "/goal" || command.starts_with("/goal ")) && controller.has_goal() {
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
            if let Err(error) = controller.goal((!text.is_empty()).then_some(text)).await {
                self.0.state.borrow_mut().pending_turn = false;
                if !self.0.state.borrow().closing {
                    self.error(&error);
                }
            }
            self.0.scheduler.request();
            return Ok(());
        }
        if command == "/undo" {
            self.clear_editor();
            self.undo(None).await;
            return Ok(());
        }
        if command == "/copy" {
            self.clear_editor();
            self.copy_last_answer();
            return Ok(());
        }
        self.clear_editor();
        self.0.state.borrow_mut().scroll = 0;
        let result = if command == "/refresh" {
            self.0.state.borrow_mut().activity = "reading your Set".into();
            controller.refresh().await
        } else if command == "/new" {
            {
                let mut state = self.0.state.borrow_mut();
                state.activity = "starting fresh".into();
                if !state.transcript.is_empty() {
                    state.transcript.add(Entry::Divider { text: "New conversation. Kumi won't use what's above".into() });
                }
                state.current = None;
                state.watching = false;
            }
            controller.new_conversation().await
        } else if command == "/reconnect" && controller.has_reconnect() {
            self.0.state.borrow_mut().activity = "connecting to Live".into();
            let result = controller.reconnect().await;
            if result.is_ok() {
                for change in &mut self.0.state.borrow_mut().changes {
                    if matches!(change.state, ChangeState::Applied | ChangeState::Unsure) {
                        change.state = ChangeState::Expired;
                        change.note = Some("Kumi reconnected to Live since, so it can't undo this; Live's own undo still can.".into());
                    }
                }
            }
            result
        } else if is_command(command) {
            self.notice(
                &format!("There's no {} command. Type / to see them.", command.split_whitespace().next().unwrap_or("")),
                NoticeTone::Info,
            );
            Ok(())
        } else {
            let text = string::trim(&self.clean(&raw, usize::MAX)).to_string();
            {
                let mut state = self.0.state.borrow_mut();
                state.transcript.add(Entry::User { text });
                state.last_sent = Some(raw.clone());
            }
            self.send(&raw).await;
            return Ok(());
        };
        if let Err(error) = result {
            self.0.state.borrow_mut().pending_turn = false;
            if !self.0.state.borrow().closing {
                self.error(&error);
            }
        }
        self.0.scheduler.request();
        Ok(())
    }
    fn clear_editor(&self) {
        self.0.state.borrow_mut().editor.clear();
    }
    pub(super) async fn send(&self, raw: &str) -> bool {
        let pinned = {
            let mut state = self.0.state.borrow_mut();
            if state.closing {
                return false;
            }
            state.activity = "thinking".into();
            state.pending_turn = true;
            state.pinned.as_ref().map(|p| p.pin.clone())
        };
        self.0.scheduler.request();
        if let Err(error) = self.0.options.controller.submit(raw, pinned).await {
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
        let aside = Rc::new(RefCell::new(Aside {
            question: self.clean(question, usize::MAX),
            answer: String::new(),
            state: "asking",
            abort: Signal::new(),
        }));
        {
            let mut state = self.0.state.borrow_mut();
            state.asides.push(aside.clone());
            if state.asides.len() > 20 {
                state.asides.remove(0);
            }
            state.panel = Some(Rc::new(RefCell::new(Panel::Btw { at: state.asides.len() - 1, scroll: 0 })));
        }
        let question = question.to_string();
        self.task(move |app| async move {
            let stream = Rc::new(RefCell::new(StreamingText::new(&app.0.state.borrow().secrets)));
            let writer = stream.clone();
            let answer = aside.clone();
            let scheduler = app.0.scheduler.clone();
            let signal = aside.borrow().abort.clone();
            let result = app
                .0
                .options
                .controller
                .aside(
                    &question,
                    Rc::new(move |text| {
                        answer.borrow_mut().answer.push_str(&writer.borrow_mut().push(&text));
                        scheduler.request();
                    }),
                    Some(signal.clone()),
                )
                .await;
            match result {
                Ok(answer) => {
                    let tail = stream.borrow_mut().finish();
                    let mut aside = aside.borrow_mut();
                    aside.answer.push_str(&tail);
                    if string::trim(&aside.answer).is_empty() {
                        aside.answer = app.clean(&answer, usize::MAX);
                    }
                    aside.state = "done";
                }
                Err(error) => {
                    let mut aside = aside.borrow_mut();
                    aside.state = "failed";
                    if !signal.is_cancelled() {
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
        self.0.tty.write(&format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(text)));
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
        if id.is_none() && !self.0.state.borrow().changes.iter().any(|c| c.state == ChangeState::Applied) {
            self.notice("There's nothing of Kumi's to undo.", NoticeTone::Info);
            return;
        }
        {
            let mut state = self.0.state.borrow_mut();
            state.undoing = true;
            state.activity = "undoing".into();
        }
        self.0.scheduler.request();
        match self.0.options.controller.undo(id).await {
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
    pub(super) fn scroll_by(&self, lines: i32) {
        let mut state = self.0.state.borrow_mut();
        state.scroll = state.scroll.saturating_add(lines).max(0);
    }
    pub(super) fn layout_for(columns: i32) -> (i32, i32) {
        let pane = if columns >= 100 { ((columns as f64 * 0.34).floor() as i32).clamp(36, 46) } else { 0 };
        (pane, columns - pane)
    }
    pub(super) fn input_width(&self) -> i32 {
        (Self::layout_for(self.0.tty.size().columns).1 - 6).max(1)
    }
}
fn enum_name(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}
pub(super) struct Command {
    pub name: &'static str,
    pub about: &'static str,
}

pub(super) const HELP: &str = "enter sends · ctrl+j or alt+enter starts a new line · ctrl+t talks instead of typing: press it again to stop, or hold it while you talk, and what you said lands in the box (enter stops and sends at once); /voice chooses the language and the microphone · ↑ and ↓ go through what you sent before · while Kumi works, enter sends a message it reads after the step under way, tab one for after the answer, and alt+↑ takes the last waiting one back · /btw asks something on the side without interrupting · esc stops Kumi · page up/down or the mouse wheel scroll, ctrl+home goes to the start and ctrl+end back · click undo in HISTORY, or /undo, to take back a change · /new starts a fresh conversation, and /conversations goes back to an earlier one · /reconnect connects to Live again, keeping the conversation · /copy copies the last answer; to select text yourself, hold Shift while dragging (Option in iTerm2) · /model and /effort choose the model and how hard it thinks; /login and /logout sign in and out · /memory shows what Kumi remembers (notes, techniques, recipes and what it learned from your Sets), and forget in MEMORY drops one; /recipes your saved ways of working · /update gets the newest Kumi · ctrl+c clears the box, then quits · type / for commands";
pub(super) const COMMANDS: &[Command] = &[
    Command { name: "/new", about: "Forget this conversation and start fresh" },
    Command { name: "/btw", about: "Ask something on the side, without interrupting Kumi" },
    Command { name: "/voice", about: "Talk instead of typing: ctrl+t, and how it listens" },
    Command { name: "/conversations", about: "Go back to an earlier conversation about this Set" },
    Command { name: "/reconnect", about: "Connect to Live again, keeping the conversation" },
    Command { name: "/undo", about: "Undo Kumi's last change" },
    Command { name: "/stop", about: "Stop Live: clips, the transport and recording" },
    Command { name: "/refresh", about: "Read your Live Set again" },
    Command { name: "/copy", about: "Copy Kumi's last answer" },
    Command { name: "/model", about: "Choose the model Kumi talks to" },
    Command { name: "/effort", about: "How hard the model thinks" },
    Command { name: "/login", about: "Sign in to a provider" },
    Command { name: "/goal", about: "Go after a sound until Kumi gets there" },
    Command { name: "/memory", about: "What Kumi remembers" },
    Command { name: "/recipes", about: "Your saved ways of working" },
    Command { name: "/logout", about: "Sign out of a provider" },
    Command { name: "/status", about: "What Kumi is connected to" },
    Command { name: "/update", about: "Get the newest Kumi" },
    Command { name: "/help", about: "Keys and commands" },
    Command { name: "/quit", about: "Close Kumi" },
];
