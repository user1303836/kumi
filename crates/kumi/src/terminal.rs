//! The plain terminal, for pipes and KUMI_UI=plain, with a native line editor.
use crate::{
    config::safe_error_message,
    history::InputHistory,
    input::{KeyInput, TerminalInput, Utf8Decoder},
    text::{library_line, remembered_line, sanitize_text, web_words, StreamingText},
    tui::{
        editor::Editor,
        keys::{InputEvent, InputParser},
        tty::TtyOutput,
        width::{cell_width, graphemes},
    },
    update::UpdateControl,
};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_agent::core::{
    commands::{self as registry, Effect, Fronts, Now, Why},
    contracts::{ConversationMatch, ConversationSummary, KeptLine, NoteChange, SessionEvent, SetAsk, SetChoice, TurnState},
    export::Format,
    goal::GoalPhase,
    goal_mode::{ObjectiveState, ObjectiveStatus},
    protocol::{op, Capabilities, SessionHandle},
};
use kumi_ai::providers::{table, Effort, ProviderId, EFFORTS};
use kumi_common::{
    js::{number, string},
    time::{now_ms_f64, perf_now, since},
};
use kumi_types::{command::KUMI, errors::RuntimeError};
use serde_json::Value;
use std::{cell::RefCell, rc::Rc, time::Duration};
pub trait Terminal {
    fn run(&self) -> LocalBoxFuture<'static, i32>;
    fn handle_event(&self, event: SessionEvent);
    fn offer_update(&self, latest: &str);
    fn interrupt(&self);
    fn close(&self) -> LocalBoxFuture<'static, i32>;
    /// Kumi follows the producer to `daw`, a DAW they opened while the session isn't connected: once nothing would be
    /// lost (no answer under way, no question or setup waiting, nothing typed), it says so and closes, to open again with
    /// that DAW. False when it can't yet: Kumi asks again later.
    fn follow_daw(&self, daw: kumi_types::daw::DawId) -> bool {
        let _ = daw;
        false
    }
    /// Whether the terminal is lent to a program Kumi runs (the producer's editor): its keys, and the signals the
    /// terminal sends for them, are that program's.
    fn lent(&self) -> bool {
        false
    }
}
#[derive(Clone)]
pub struct TerminalOptions {
    /// The session, reached only through its protocol: its models with it.
    pub session: Rc<dyn SessionHandle>,
    pub input: Rc<dyn TerminalInput>,
    pub output: Rc<dyn TtyOutput>,
    pub mode: String,
    pub startup_notice: Option<String>,
    pub secrets: Vec<String>,
    pub close_timeout_ms: Option<u64>,
    pub history: Option<Rc<RefCell<InputHistory>>>,
    pub updates: Option<UpdateControl>,
    /// /daw <name>: Kumi closes and opens again with that DAW. None where Live is the only DAW here.
    pub switch_daw: Option<Rc<dyn Fn(kumi_types::daw::DawId)>>,
    /// Kumi follows the producer to a DAW they opened ([`Terminal::follow_daw`]): it closes and opens again with it.
    /// None where the producer asked for a DAW.
    pub follow_daw: Option<Rc<dyn Fn(kumi_types::daw::DawId)>>,
    /// /willington, where Kumi's bridge can carry Willington.
    pub willington: Option<kumi_daw::provider::Toggle>,
    /// What's new since the producer last opened Kumi, said once as it starts.
    pub whats_new: Option<crate::whats_new::News>,
    /// Fixed phrases ("tempo 124", "mute drums") change Live at once, with no model call (`instantPhrases`).
    pub instant: bool,
    /// How to start Kumi next time, said after the farewell when this window can't yet ([`crate::install::next_time`]).
    pub next_time: Option<String>,
    /// The producer chooses a kept conversation as Kumi starts (`kumi -r`): the list, with these words, is said first.
    pub choose_conversation: Option<String>,
    /// What /export and /share take out: `kumi report`'s redactor, made at each export. None takes out what `secrets`
    /// holds, the home folder and the account name.
    pub redact: Option<crate::export::MakeRedactor>,
    /// Where /export and /share save; None saves in the folder Kumi was started in.
    pub folder: Option<std::path::PathBuf>,
    /// How the producer runs Kumi from a shell ("kumi"), where Kumi's words name one of its commands.
    pub command: String,
}
impl TerminalOptions {
    pub fn new(session: Rc<dyn SessionHandle>, input: Rc<dyn TerminalInput>, output: Rc<dyn TtyOutput>, mode: &str) -> Self {
        Self {
            session,
            input,
            output,
            mode: mode.into(),
            startup_notice: None,
            secrets: vec![],
            close_timeout_ms: None,
            history: None,
            updates: None,
            switch_daw: None,
            follow_daw: None,
            willington: None,
            whats_new: None,
            instant: true,
            next_time: None,
            choose_conversation: None,
            redact: None,
            command: "kumi".into(),
            folder: None,
        }
    }
}
/// A command by the shared table's rule ([`registry::is_command`]), so a line that starts with a path (a file dragged in)
/// is a message; or a lone `/`, which opens the menu in the full-screen app and never reaches the model, so here it's
/// pointed to /help.
fn command_like(line: &str) -> bool {
    line == "/" || registry::is_command(line)
}
/// A session that can do everything, in a front end with every part: what each command in the table comes to, at most.
fn all_on() -> Now {
    let mut can = serde_json::to_value(Capabilities::default()).unwrap_or_default();
    for value in can.as_object_mut().into_iter().flat_map(|can| can.values_mut()) {
        *value = Value::Bool(true);
    }
    let fronts = Fronts { voice: true, updates: true, switch_daw: true, willington: true };
    Now { busy: false, goal_running: false, can: serde_json::from_value(can).unwrap_or_default(), fronts }
}
/// Plain mode's words for a command where the table's are the full-screen app's: plain mode takes words where that app
/// opens a panel, and only lists what that app also changes.
fn plain_about(name: &str) -> Option<String> {
    Some(match name {
        "/model" => "The model Kumi talks to: /model <provider> lists a provider's, /model <provider>/<model> chooses one".into(),
        "/effort" => "How hard the model thinks: /effort <level>, or /effort default".into(),
        "/login" => format!("Sign in to a provider, from a shell: {} login <provider>", *KUMI),
        "/logout" => "Sign out of a provider: /logout <provider>".into(),
        "/export" => "This conversation as a file: /export (Markdown), /export html, or /export json (for another Kumi)".into(),
        "/jobs" => "What's running in the background, a line each".into(),
        "/music" => "Your own music, which Kumi learns your taste from: listed here, changed in the full-screen app".into(),
        _ => return None,
    })
}
/// What the full-screen app shows and plain mode can't: /help names them, and a line asking for one is told so.
fn full_screen_only(effect: &Effect) -> bool {
    matches!(effect, Effect::Voice | Effect::Map | Effect::Palette | Effect::Copy | Effect::Recipe | Effect::TakeBack { .. })
}
/// A command in the table that only the full-screen app runs, whatever this session has or is doing.
fn the_apps_own(name: &str) -> bool {
    registry::named(name).is_some_and(|command| full_screen_only(&registry::run(command.name, &all_on())))
}
fn head(text: &str, limit: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(limit).collect::<Vec<_>>())
}
fn enum_name(value: impl serde::Serialize) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}
fn position(text: &str, columns: i32) -> (usize, i32) {
    let (mut row, mut column) = (0, 0);
    for g in graphemes(text) {
        if g == "\n" {
            row += 1;
            column = 0;
            continue;
        }
        let width = cell_width(g);
        if width == 0 {
            continue;
        }
        if column + width > columns {
            row += 1;
            column = 0;
        }
        column += width;
    }
    (row, column)
}
struct Presentation {
    tail: String,
    prefix: String,
    pipe_line_open: bool,
    cursor_row: usize,
    editor: Editor,
    history: Vec<String>,
    history_at: Option<usize>,
    draft: String,
    tty: bool,
}
impl Presentation {
    fn new(tty: bool, history: Vec<String>) -> Self {
        Self {
            tail: String::new(),
            prefix: "assistant> ".into(),
            pipe_line_open: false,
            cursor_row: 0,
            editor: Editor::new(),
            history,
            history_at: None,
            draft: String::new(),
            tty,
        }
    }
    fn frame(&mut self, out: &dyn TtyOutput, stable: &str, partial: &str, restore: bool) {
        if !self.tty {
            out.write(&format!("{stable}{partial}"));
            return;
        }
        let columns = out.columns().unwrap_or(80).max(2);
        let up = if self.cursor_row > 0 { format!("\x1b[{}A", self.cursor_row) } else { String::new() };
        out.write(&format!("\r{up}\x1b[0J{stable}{}", if partial.is_empty() { String::new() } else { format!("{partial}\n") }));
        self.cursor_row = 0;
        if !restore {
            return;
        }
        let text = self.editor.text();
        let before = graphemes(&text).into_iter().take(self.editor.cursor()).collect::<String>();
        let current = format!("kumi> {text}");
        let (end_row, _) = position(&current, columns);
        let (cursor_row, cursor_col) = position(&format!("kumi> {before}"), columns);
        out.write(&current);
        out.write(&format!(
            "\r{}{}",
            if end_row > cursor_row { format!("\x1b[{}A", end_row - cursor_row) } else { String::new() },
            if cursor_col > 0 { format!("\x1b[{cursor_col}C") } else { String::new() }
        ));
        self.cursor_row = if partial.is_empty() { 0 } else { position(partial, columns).0 + 1 } + cursor_row;
    }
    fn redraw(&mut self, out: &dyn TtyOutput) {
        let partial = if self.tail.is_empty() { String::new() } else { format!("{}{}", self.prefix, self.tail) };
        self.frame(out, "", &partial, true);
    }
    fn text(&mut self, out: &dyn TtyOutput, text: &str) {
        if text.is_empty() {
            return;
        }
        if !self.tty {
            out.write(&format!("{}{text}", if self.pipe_line_open { "" } else { &self.prefix }));
            self.pipe_line_open = !text.ends_with('\n');
            self.prefix.clear();
            return;
        }
        self.tail.push_str(text);
        let parts: Vec<_> = self.tail.split('\n').collect();
        let stable = parts[..parts.len() - 1]
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}{s}\n", if i == 0 { self.prefix.as_str() } else { "" }))
            .collect::<String>();
        let had = parts.len() > 1;
        self.tail = parts.last().unwrap().to_string();
        if had {
            self.prefix.clear();
        }
        let partial = if self.tail.is_empty() { String::new() } else { format!("{}{}", self.prefix, self.tail) };
        self.frame(out, &stable, &partial, true);
    }
    fn notice(&mut self, out: &dyn TtyOutput, text: &str, final_line: bool) {
        if !self.tty {
            out.write(&format!("{}{text}\n", if self.pipe_line_open { "\n" } else { "" }));
            self.pipe_line_open = false;
        } else {
            let stable =
                format!("{}{text}\n", if self.tail.is_empty() { String::new() } else { format!("{}{}\n", self.prefix, self.tail) });
            self.frame(out, &stable, "", !final_line);
            self.tail.clear();
        }
        self.prefix = "assistant> ".into();
    }
    fn commit(&mut self, out: &dyn TtyOutput) -> String {
        let text = self.editor.text();
        if self.tty {
            let stable =
                format!("{}kumi> {text}\n", if self.tail.is_empty() { String::new() } else { format!("{}{}\n", self.prefix, self.tail) });
            self.frame(out, &stable, "", false);
        }
        if !text.is_empty() {
            self.history.retain(|s| s != &text);
            self.history.insert(0, text.clone());
            self.history.truncate(500);
        }
        self.history_at = None;
        self.draft.clear();
        self.editor.clear();
        self.tail.clear();
        self.prefix = "assistant> ".into();
        text
    }
    fn recall(&mut self, up: bool) {
        if up {
            let at = self.history_at.map_or(0, |at| at + 1);
            if at >= self.history.len() {
                return;
            }
            if self.history_at.is_none() {
                self.draft = self.editor.text();
            }
            self.history_at = Some(at);
            self.editor.set(&self.history[at]);
        } else if let Some(at) = self.history_at {
            if at == 0 {
                self.history_at = None;
                self.editor.set(&self.draft);
            } else {
                self.history_at = Some(at - 1);
                self.editor.set(&self.history[at - 1]);
            }
        }
    }
}
struct State {
    /// The web page saved last for this conversation, with its key: /share says it again while nothing changed.
    shared: Option<(u64, std::path::PathBuf)>,
    started: bool,
    closing: bool,
    /// Kumi said goodbye: nothing more is said after it (a command still under way when /quit came).
    closed: bool,
    cancelling: bool,
    newer: Option<String>,
    taste: Vec<KeptLine>,
    told_library: bool,
    told_music: bool,
    suppress: bool,
    displayed_bytes: usize,
    started_at: f64,
    first_text_ms: Option<f64>,
    queued: Option<String>,
    answering: bool,
    output_failed: bool,
    text: StreamingText,
    presentation: Option<Presentation>,
    decoder: Utf8Decoder,
    pipe_buffer: String,
    after_cr: bool,
    was_raw: bool,
    /// Changes whose undo just ended for one reason (Live opened another Set): the first is said as it comes, the
    /// rest of them in one line after it, not one each (#254). The reason, and the titles after the first.
    expired: Option<(String, Vec<String>)>,
    /// Background jobs that ended whose request carries on, once Kumi is idle.
    carry: Vec<u32>,
    /// Each change's state as last said: a change told again as it was (its picture came after it) isn't said twice.
    said: std::collections::HashMap<String, String>,
    /// Kumi's question about another Set's conversation, open until a line says 1, 2 or 3 (or Kumi withdraws it).
    asking: Option<String>,
    /// Why the DAW answers late, while it does (a dialog is open), as its focus says.
    busy: Option<String>,
    /// The last /goal as its event said it (its words, state, turns and budget): what /goal says when the session says
    /// nothing more, and whether one is running. None once it's cleared, or a sound match took over.
    objective: Option<ObjectiveStatus>,
    /// A sound match is running or starting.
    matching: bool,
    /// Lines said and turns begun since Kumi started: whether a command's call said anything itself.
    said_lines: u64,
    turns_begun: u64,
    /// The /slots swap under way, by its request's id: one at a time, and Ctrl-C stops it, as Esc does in the full-screen
    /// app. Each /slots request and aside has an id of its own.
    slots: Option<u32>,
    next_id: u32,
}
struct Inner {
    options: TerminalOptions,
    state: RefCell<State>,
    parser: RefCell<Option<InputParser>>,
    keys: RefCell<Option<Rc<KeyInput>>>,
    done: tokio::sync::watch::Sender<Option<i32>>,
    /// Commands still saying something (a /status working out its conversation): the goodbye waits for them.
    saying: tokio::sync::watch::Sender<usize>,
}
#[derive(Clone)]
pub struct PlainTerminal(Rc<Inner>);
pub fn create_terminal(options: TerminalOptions) -> PlainTerminal {
    let text = StreamingText::new(&options.secrets);
    let terminal = PlainTerminal(Rc::new(Inner {
        options,
        state: RefCell::new(State {
            shared: None,
            started: false,
            closing: false,
            closed: false,
            cancelling: false,
            newer: None,
            taste: vec![],
            told_library: false,
            told_music: false,
            suppress: false,
            displayed_bytes: 0,
            started_at: perf_now(),
            first_text_ms: None,
            queued: None,
            answering: false,
            output_failed: false,
            text,
            presentation: None,
            decoder: Utf8Decoder::new(),
            pipe_buffer: String::new(),
            after_cr: false,
            was_raw: false,
            expired: None,
            carry: vec![],
            said: std::collections::HashMap::new(),
            asking: None,
            busy: None,
            objective: None,
            matching: false,
            said_lines: 0,
            turns_begun: 0,
            slots: None,
            next_id: 0,
        }),
        parser: RefCell::new(None),
        keys: RefCell::new(None),
        done: tokio::sync::watch::channel(None).0,
        saying: tokio::sync::watch::channel(0).0,
    }));
    // What the session says between its replies (/slots' progress) comes in as its other events do.
    let weak = Rc::downgrade(&terminal.0);
    terminal.0.options.session.listen(Rc::new(move |event| {
        if let Some(inner) = weak.upgrade() {
            PlainTerminal(inner).handle_event(event);
        }
    }));
    terminal
}
impl PlainTerminal {
    fn done(&self) -> LocalBoxFuture<'static, i32> {
        let mut receive = self.0.done.subscribe();
        async move {
            loop {
                if let Some(code) = *receive.borrow() {
                    return code;
                }
                if receive.changed().await.is_err() {
                    return 1;
                }
            }
        }
        .boxed_local()
    }
    /// What's new, a line each: the heading, the notes, then `footer`.
    fn news(&self, news: &crate::whats_new::News, footer: Option<String>) {
        self.notice(&format!("[new] {}:", news.title));
        for item in &news.items {
            self.notice(&format!("[new] · {item}"));
        }
        if let Some(footer) = footer {
            self.notice(&format!("[new] {footer}"));
        }
    }
    /// Carry on with a job whose request needs it, now that Kumi is idle.
    fn carry_on(&self, id: u32) {
        let this = self.clone();
        tokio::task::spawn_local(async move {
            if this.session().op(op::CarryOn { job: id }).await.unwrap_or(false) {
                this.0.state.borrow_mut().carry.retain(|waiting| *waiting != id);
            }
        });
    }
    fn notice(&self, message: &str) {
        let mut state = self.0.state.borrow_mut();
        if state.output_failed || state.closed {
            return;
        }
        state.said_lines += 1;
        let text = head(&sanitize_text(message, &self.0.options.secrets).replace('\n', " "), 2048);
        if let Some(p) = &mut state.presentation {
            p.notice(self.0.options.output.as_ref(), &text, false);
        }
    }
    fn error(&self, error: &RuntimeError) {
        self.notice(&format!("[error] {}", safe_error_message(Some(&error.message()), &self.0.options.secrets)));
    }
    /// The session, reached only through its protocol.
    fn session(&self) -> &dyn SessionHandle {
        &*self.0.options.session
    }
    /// The model Kumi talks to, as the session says it; None where there's no model to choose.
    fn current_model(&self) -> Option<kumi_ai::control::CurrentModel> {
        self.session().models().map(|models| models.current)
    }
    /// An id for a request that its own events and stop name (an aside, a /slots one).
    fn next_id(&self) -> u32 {
        let mut state = self.0.state.borrow_mut();
        state.next_id += 1;
        state.next_id
    }
    fn busy(&self) -> bool {
        matches!(self.session().status().state, TurnState::Running | TurnState::Cancelling)
    }
    fn prompt(&self) {
        let mut state = self.0.state.borrow_mut();
        if state.closing || state.output_failed {
            return;
        }
        if let Some(p) = &mut state.presentation {
            if p.tty {
                p.redraw(self.0.options.output.as_ref());
            }
        }
    }
    fn finish(&self, code: i32) -> LocalBoxFuture<'static, i32> {
        let mut state = self.0.state.borrow_mut();
        if state.closing {
            return self.done();
        }
        state.closing = true;
        state.suppress = true;
        state.text.discard();
        drop(state);
        let this = self.clone();
        tokio::task::spawn_local(async move {
            let mut code = code;
            let result = tokio::time::timeout(
                Duration::from_millis(this.0.options.close_timeout_ms.unwrap_or(6000)),
                this.session().op(op::Close {}),
            )
            .await
            .unwrap_or_else(|_| Err(RuntimeError::plain("Shutdown deadline exceeded")));
            let hint = this.session().op(op::ConversationNow {}).await.ok().flatten();
            // What a command under way still has to say comes before the goodbye (a little while at most).
            let mut saying = this.0.saying.subscribe();
            let _ = tokio::time::timeout(Duration::from_millis(2000), saying.wait_for(|count| *count == 0)).await;
            if result.is_err() {
                code = 1;
            }
            {
                let mut state = this.0.state.borrow_mut();
                if !state.output_failed {
                    let text = match result {
                        Ok(()) => {
                            let lines: Vec<String> =
                                [Some(crate::tui::app::farewell()), crate::tui::app::continue_hint(hint), this.0.options.next_time.clone()]
                                    .into_iter()
                                    .flatten()
                                    .collect();
                            lines.join("\n")
                        }
                        Err(error) => {
                            code = 1;
                            head(&format!("[error] {}", safe_error_message(Some(&error.message()), &this.0.options.secrets)), 2048)
                        }
                    };
                    if let Some(p) = &mut state.presentation {
                        p.notice(this.0.options.output.as_ref(), &text, true);
                    }
                }
                state.closed = true;
            }
            if let Some(parser) = this.0.parser.borrow_mut().take() {
                parser.dispose();
            }
            if let Some(keys) = this.0.keys.borrow_mut().take() {
                keys.pause();
            }
            let was_raw = this.0.state.borrow().was_raw;
            let _ = this.0.options.input.set_raw_mode(was_raw);
            this.0.options.input.pause();
            this.0.done.send_replace(Some(code));
        });
        self.done()
    }
    fn spawn_submit(&self, text: String) {
        // Every line is cleaned, typed or piped: a person's paste can arrive through a pipe too (`pbpaste | kumi`, or
        // typing at a terminal while the output goes to `tee`).
        let text = sanitize_text(&text, &[]);
        // A readline line listener starts its async handler immediately, before EOF can close it.
        let this = self.clone();
        let mut task = Box::pin(async move {
            if let Err(error) = this.submitted(&text).await {
                if !this.0.state.borrow().closing {
                    this.error(&error);
                }
            }
        });
        let mut cx = std::task::Context::from_waker(futures::task::noop_waker_ref());
        if std::future::Future::poll(task.as_mut(), &mut cx).is_pending() {
            tokio::task::spawn_local(task);
        }
    }
    fn typed(&self, event: InputEvent) {
        if self.0.state.borrow().closing {
            return;
        }
        match event {
            InputEvent::Key { name, mods, .. } if mods.ctrl && name == "c" => self.interrupt(),
            InputEvent::Key { name, mods, .. } if name == "enter" || (mods.ctrl && name == "j") => {
                let text = {
                    let mut state = self.0.state.borrow_mut();
                    state.presentation.as_mut().map(|p| p.commit(self.0.options.output.as_ref())).unwrap_or_default()
                };
                self.spawn_submit(text);
            }
            InputEvent::Key { name, mods, .. } if mods.ctrl && name == "d" => {
                let empty = self.0.state.borrow().presentation.as_ref().is_none_or(|p| p.editor.is_empty());
                if empty {
                    drop(self.finish(0));
                } else {
                    self.edit(|p| p.editor.delete());
                }
            }
            // Cleaned as the full-screen composer cleans it, so no terminal control in a paste is echoed back. Typing is a
            // run, so the line's editor keeps one step for it, not one for each key.
            InputEvent::Text { text } => self.edit(|p| p.editor.type_text(&sanitize_text(&text, &[]))),
            InputEvent::Paste { text } => self.edit(|p| p.editor.insert(&sanitize_text(&text, &[]))),
            InputEvent::Key { name, mods, .. } => self.edit(|p| match (name.as_str(), mods.ctrl, mods.alt) {
                ("left", false, false) => p.editor.left(),
                ("right", false, false) => p.editor.right(),
                ("left", true, _) | ("b", _, true) => p.editor.word_left(),
                ("right", true, _) | ("f", _, true) => p.editor.word_right(),
                ("a", true, _) | ("home", _, _) => p.editor.home(),
                ("e", true, _) | ("end", _, _) => p.editor.end(),
                ("k", true, _) => p.editor.kill_to_end(),
                ("u", true, _) => p.editor.kill_to_start(),
                ("w", true, _) | ("backspace", _, true) => p.editor.delete_word_left(),
                ("backspace", _, _) => p.editor.backspace(),
                ("delete", _, _) => p.editor.delete(),
                ("up", _, _) | ("p", true, _) => p.recall(true),
                ("down", _, _) | ("n", true, _) => p.recall(false),
                ("b", true, _) => p.editor.left(),
                ("f", true, _) => p.editor.right(),
                _ => {}
            }),
            _ => {}
        }
    }
    fn edit(&self, edit: impl FnOnce(&mut Presentation)) {
        let mut state = self.0.state.borrow_mut();
        if let Some(p) = &mut state.presentation {
            edit(p);
            p.redraw(self.0.options.output.as_ref());
        }
    }
    fn bytes(&self, bytes: &[u8]) {
        let text = self.0.state.borrow_mut().decoder.write(bytes);
        if let Some(parser) = self.0.parser.borrow().clone() {
            parser.push(&text);
            return;
        }
        let mut lines = vec![];
        {
            let mut state = self.0.state.borrow_mut();
            for c in text.chars() {
                if c == '\n' && state.after_cr {
                    state.after_cr = false;
                    continue;
                }
                state.after_cr = c == '\r';
                if c == '\r' || c == '\n' {
                    lines.push(std::mem::take(&mut state.pipe_buffer));
                } else {
                    state.pipe_buffer.push(c);
                }
            }
        }
        for line in lines {
            self.spawn_submit(line);
        }
    }
    async fn submitted(&self, input: &str) -> Result<(), RuntimeError> {
        if self.0.state.borrow().closing {
            return Ok(());
        }
        // Readline already committed a TTY line; pipe lines need the same answer-prefix reset.
        if let Some(presentation) = &mut self.0.state.borrow_mut().presentation {
            if !presentation.tty {
                presentation.tail.clear();
                presentation.prefix = "assistant> ".into();
            }
        }
        let command = string::trim(input);
        if command.is_empty() {
            self.prompt();
            return Ok(());
        }
        if let Some(history) = &self.0.options.history {
            history.borrow_mut().add(input);
        }
        // The answer to Kumi's question about another Set's conversation: 1, 2 or 3. A command runs as usual, with the
        // question still open; anything else asks it again.
        let asking = self.0.state.borrow().asking.clone();
        if let Some(question) = asking.filter(|_| !command_like(command)) {
            let choice = match command {
                "1" => SetChoice::Open,
                "2" => SetChoice::Copy,
                "3" => SetChoice::Cancel,
                _ => {
                    self.notice(&question);
                    self.prompt();
                    return Ok(());
                }
            };
            self.0.state.borrow_mut().asking = None;
            self.session().op(op::AnswerSet { choice }).await?;
            self.prompt();
            return Ok(());
        }
        // A lone `/` opens the menu in the full-screen app and never reaches the model; here it's pointed to /help.
        if command == "/" {
            self.notice("Unknown command. Use /help.");
            if !self.busy() {
                self.prompt();
            }
            return Ok(());
        }
        let mut parts = command.split(|c: char| c.is_whitespace() || c == '\u{feff}').filter(|s| !s.is_empty());
        let verb = parts.next().unwrap_or("");
        let argument = parts.next();
        // Plain mode's own commands, which the shared table doesn't have: a note pinned, unpinned or forgotten.
        let own = matches!(verb, "/pin" | "/unpin" | "/forget");
        // The table answers for /model, /effort, /login and /logout alone: plain mode reads their words itself (/model
        // <provider>, /effort high, /logout <provider>), where the full-screen app chooses in a panel.
        let asked = if matches!(verb, "/model" | "/effort" | "/login" | "/logout") { verb } else { command };
        let effect = (!own).then(|| registry::run(asked, &self.now()));
        if self.busy() {
            match effect {
                // A message while Kumi gets ready waits, once; while it answers, it's said to wait.
                Some(Effect::Message) => {
                    let mut state = self.0.state.borrow_mut();
                    if !state.answering && state.queued.is_none() {
                        state.queued = Some(input.into());
                        drop(state);
                        self.notice("[waiting] Kumi is getting ready; your message goes as soon as it is.");
                    } else {
                        drop(state);
                        self.notice("[busy] Busy; cancel first. No second turn was submitted.");
                    }
                }
                None => self.notice("[busy] Busy; cancel first. No second turn was submitted."),
                Some(effect) => {
                    if let Err(error) = self.effect(effect, verb, argument, command, input).await {
                        if !self.0.state.borrow().closing {
                            self.error(&error);
                        }
                    }
                }
            }
            return Ok(());
        }
        let result = match effect {
            Some(effect) => self.effect(effect, verb, argument, command, input).await,
            None => self.command(verb, argument, command, input).await,
        };
        if !self.0.state.borrow().closing {
            if let Err(error) = &result {
                self.error(error);
            }
            self.prompt();
        }
        Ok(())
    }
    /// /help, a line each, from the shared table: each command this session has, in the table's words or plain mode's where
    /// they differ; plain mode's own; those only the full-screen app shows; and the keys.
    fn help(&self) -> Vec<String> {
        let now = Now { busy: false, ..self.now() };
        let (words, live) = (crate::daw::words(), crate::daw::current().id() == kumi_types::daw::DawId::Ableton);
        let (mut lines, mut there) = (vec![], vec![]);
        for command in registry::COMMANDS.iter().filter(|command| command.offered(&now.can, &now.fronts)) {
            if full_screen_only(&registry::run(command.name, &now)) {
                there.push(command.name);
            } else {
                let about = plain_about(command.name).unwrap_or_else(|| command.about(words, live));
                lines.push(format!("[help] {}: {about}", command.name));
            }
        }
        if now.can.change_note {
            lines.push("[help] /pin <id>: Keep a note from /memory even when Kumi's memory is full".into());
            lines.push("[help] /unpin <id>: Unpin a note".into());
        }
        if now.can.forget || now.can.forget_taste || now.can.forget_technique {
            lines.push("[help] /forget <id>: Forget a note, a technique, or u1, u2… from your places".into());
        }
        if !there.is_empty() {
            lines.push(format!("[help] Only in the full-screen app: {}", there.join(", ")));
        }
        lines.push(format!("[help] Ctrl-C: cancel work; idle: exit. EOF exits. Sign in with: {} login <provider>.", *KUMI));
        lines
    }
    /// What the shared table needs of this session and of plain mode to answer a line.
    fn now(&self) -> Now {
        let options = &self.0.options;
        let fronts = Fronts {
            voice: false,
            updates: options.updates.is_some(),
            switch_daw: options.switch_daw.is_some(),
            willington: options.willington.is_some(),
        };
        let can = options.session.capabilities();
        let goal_running = {
            let state = self.0.state.borrow();
            state.matching || state.objective.as_ref().is_some_and(|goal| goal.state == ObjectiveState::Running)
        };
        Now { busy: self.busy(), goal_running, can, fronts }
    }
    /// Carries out what the shared table made of a line, as text; what only the full-screen app can show says so.
    async fn effect(&self, effect: Effect, verb: &str, argument: Option<&str>, command: &str, input: &str) -> Result<(), RuntimeError> {
        let session = self.session();
        let words = string::trim(command.strip_prefix(verb).unwrap_or("")).to_string();
        match effect {
            Effect::Quit => {
                self.finish(0).await;
            }
            Effect::Help => {
                for line in self.help() {
                    self.notice(&line);
                }
            }
            Effect::Daw => {
                let Some(switch) = self.0.options.switch_daw.clone() else { return Ok(()) };
                match crate::daw::daw_command(&command["/daw".len()..]) {
                    crate::daw::DawCommand::Say(words) => self.notice(&format!("[daw] {words}")),
                    crate::daw::DawCommand::Switch(_, _) if self.busy() => {
                        self.notice("[daw] Kumi is working; /daw once it's done (Ctrl-C stops it).");
                    }
                    crate::daw::DawCommand::Switch(daw, words) => {
                        self.notice(&format!("[daw] {words}"));
                        switch(daw);
                        self.finish(0).await;
                    }
                }
            }
            Effect::Changelog => {
                let since = self.0.options.whats_new.as_ref().and_then(|news| news.since.clone());
                match crate::whats_new::changelog(kumi_types::version::KUMI_VERSION, since.as_deref()) {
                    Some(news) => self.news(&news, Some(format!("Every release: {}", crate::whats_new::history_link()))),
                    None => self.notice("[new] Kumi's changelog has no notes for this version."),
                }
            }
            Effect::Update => {
                if let Some(updates) = &self.0.options.updates {
                    if self.busy() {
                        self.notice("[update] Kumi is working; /update once it's done (Ctrl-C stops it).");
                        return Ok(());
                    }
                    let known = self.0.state.borrow().newer.clone();
                    let latest = if known.is_some() {
                        known
                    } else {
                        match (updates.check)().await {
                            Ok(latest) => latest,
                            Err(error) => {
                                self.notice(&format!(
                                    "[update] {}. Try /update again later.",
                                    safe_error_message(Some(&error.message()), &self.0.options.secrets)
                                ));
                                return Ok(());
                            }
                        }
                    };
                    let Some(latest) = latest else {
                        self.notice(&format!("[update] Kumi is up to date ({}).", updates.current));
                        return Ok(());
                    };
                    self.notice(&format!("[update] Updating to Kumi {latest}: Kumi closes, updates and opens again."));
                    (updates.request)();
                    self.finish(0).await;
                }
            }
            Effect::Stop => {
                if !session.capabilities().stop_daw {
                    self.notice(&format!("[stop] Kumi can't stop {} here.", crate::daw::words().app));
                    return Ok(());
                }
                if self.busy() {
                    let _ = session.op(op::Cancel {}).await;
                }
                if !session.op(op::StopDaw {}).await? {
                    self.notice(&format!("[stop] Kumi couldn't stop {0} just now; press space in {0}.", crate::daw::words().app));
                }
            }
            Effect::Status => {
                let status = session.status();
                let library = library_line(session.library().as_ref());
                self.notice(&format!(
                    "[status] {}; MCP/{}: {}; turns {}{}; {}{}",
                    enum_name(status.state),
                    crate::daw::words().app,
                    self.0.state.borrow().busy.as_ref().map_or_else(|| enum_name(status.connection), |why| format!("busy ({why})")),
                    status.turns,
                    status.max_turns.filter(|n| *n != 0).map(|n| format!("/{n}")).unwrap_or_default(),
                    status.observation.clone().unwrap_or_else(|| format!("No current {} observation", crate::daw::words().app)),
                    library.map(|s| format!("; {s}")).unwrap_or_default()
                ));
                // Its second line comes before Kumi's goodbye, should /quit come while it works out the conversation.
                struct Saying(PlainTerminal);
                impl Drop for Saying {
                    fn drop(&mut self) {
                        self.0 .0.saying.send_modify(|count| *count -= 1);
                    }
                }
                self.0.saying.send_modify(|count| *count += 1);
                let _saying = Saying(self.clone());
                if let Some(line) = remembered_line(session.op(op::Remembered {}).await.ok().flatten()) {
                    self.notice(&format!("[status] {line}"));
                }
                if let Some(now) = session.op(op::ConversationNow {}).await.ok().flatten() {
                    let secrets = self.0.options.secrets.clone();
                    self.notice(&format!(
                        "[status] {}",
                        crate::tui::app::conversation_line(&now, |text| head(&sanitize_text(text, &secrets), 60))
                    ));
                }
            }
            // Going back to a conversation, or carrying on in a copy, waits until Kumi is done, as in the full-screen app;
            // the list still shows.
            Effect::Resume { word } if self.busy() && !word.is_empty() => self.wait_for(verb),
            Effect::Fork { .. } if self.busy() => self.wait_for(verb),
            // (/model's words are plain mode's own: /model <provider> lists, /model <provider>/<model> chooses)
            Effect::Model { .. }
            | Effect::Effort
            | Effect::Fast
            | Effect::Login
            | Effect::Logout
            | Effect::Memory
            | Effect::Recipes
            | Effect::Note
            | Effect::Resume { .. }
            | Effect::Rename { .. }
            | Effect::Fork { .. }
            | Effect::Export { .. }
            | Effect::Share
            | Effect::Willington
            | Effect::Undo
            | Effect::Refresh
            | Effect::Reconnect
            | Effect::New { .. }
            | Effect::Message => self.command(verb, argument, command, input).await?,
            // The line itself is the loop's request; the session reads it.
            Effect::Loop => self.turn(input).await?,
            Effect::Goal => {
                let before = self.said();
                self.0.state.borrow_mut().answering = true;
                let result = session.op(op::Goal { text: (!words.is_empty()).then(|| words.clone()) }).await;
                self.0.state.borrow_mut().answering = false;
                result?;
                // Showing it, its status or an edit, the session answers only with the goal's event, which the full-screen
                // app draws in its GOAL tab: here it's said.
                if self.said() == before {
                    self.say_goal();
                }
            }
            Effect::PauseGoal | Effect::PauseLoop if self.busy() => {
                let _ = session.op(op::Cancel {}).await;
            }
            Effect::PauseGoal => self.notice("[goal] There's no goal running to pause."),
            Effect::PauseLoop => self.notice("[loop] There's no loop running to pause."),
            Effect::StopGoal => {
                let before = self.said();
                if !session.op(op::StopGoal {}).await? {
                    self.notice("[goal] There's no goal to stop.");
                } else if self.said() == before {
                    self.notice("[goal] Stopped.");
                }
            }
            Effect::StopLoop => {
                if !(session.capabilities().stop_loop && session.op(op::StopLoop {}).await?) {
                    if self.busy() {
                        let _ = session.op(op::Cancel {}).await;
                    } else {
                        self.notice("[loop] There's no loop to stop.");
                    }
                }
            }
            Effect::ShowGoal => self.notice("[goal] A goal is running: /goal pause or /goal stop."),
            Effect::Btw => self.aside(&words).await?,
            Effect::Jobs => self.jobs(),
            Effect::Slots => self.slots(&words).await,
            Effect::Music => self.music(&words).await,
            // (the effects `full_screen_only` names)
            Effect::Voice | Effect::Map | Effect::Palette | Effect::Copy | Effect::Recipe | Effect::TakeBack { .. } => {
                self.notice(&format!("[plain] {verb} isn't available in plain mode: the full-screen app has it."))
            }
            Effect::Refused(Why::NotKept) => self.notice(&format!("[conversations] {}", crate::tui::app::not_kept())),
            // Told to wait only for what this session and plain mode can do once Kumi is done.
            Effect::Refused(Why::Busy { verb }) if self.offers(&verb) && !the_apps_own(&verb) => self.wait_for(&verb),
            Effect::Refused(Why::Busy { verb: name }) | Effect::Unknown { name } => self.not_here(&name),
        }
        Ok(())
    }
    /// `verb` waits until Kumi is done.
    fn wait_for(&self, verb: &str) {
        self.notice(&format!("[busy] Kumi is working: {verb} once it's done (Ctrl-C stops it)."));
    }
    /// How much has been said, in lines and turns begun: a call that changes neither said nothing itself.
    fn said(&self) -> (u64, u64) {
        let state = self.0.state.borrow();
        (state.said_lines, state.turns_begun)
    }
    /// The goal as its last event said it: its words, state, turns and budget, and what carries it on.
    fn say_goal(&self) {
        let goal = self.0.state.borrow().objective.clone();
        let Some(goal) = goal else {
            self.notice("[goal] No goal yet: /goal <what to reach> sets one.");
            return;
        };
        let words = head(&sanitize_text(&goal.objective, &self.0.options.secrets), 120);
        let mut said = format!("[goal] {words} · {} · {} of {} turns", enum_name(goal.state), goal.turns, goal.turn_budget);
        if let Some(reason) = goal.reason.as_deref().filter(|reason| !reason.is_empty()) {
            said.push_str(&format!(" · {}", head(&sanitize_text(reason, &self.0.options.secrets), 160)));
        }
        if goal.state == ObjectiveState::Paused {
            said.push_str(" · /goal resume carries on");
        }
        self.notice(&said);
    }
    /// Whether this session and plain mode have `name`, as /help lists them.
    fn offers(&self, name: &str) -> bool {
        let now = self.now();
        registry::named(name).is_some_and(|command| command.offered(&now.can, &now.fronts))
    }
    /// A command this session or plain mode lacks, or words it doesn't take: /stop says why, as plain mode always has,
    /// and the full-screen app's own are named as such; any other is unknown here.
    fn not_here(&self, name: &str) {
        if name == "/stop" && !self.offers(name) {
            self.notice(&format!("[stop] Kumi can't stop {} here.", crate::daw::words().app));
        } else if the_apps_own(name) {
            self.notice(&format!("[plain] {name} isn't available in plain mode: the full-screen app has it."));
        } else {
            self.notice("Unknown command. Use /help.");
        }
    }
    /// A turn of `input` for the model, as a message is sent.
    async fn turn(&self, input: &str) -> Result<(), RuntimeError> {
        self.0.state.borrow_mut().answering = true;
        let result = self.session().op(op::Submit { input: input.into(), pinned: None, attachments: vec![], references: vec![] }).await;
        self.0.state.borrow_mut().answering = false;
        result
    }
    /// /btw <question>: asked on the side, without stopping what Kumi is doing; its answer is said once it's all in.
    async fn aside(&self, question: &str) -> Result<(), RuntimeError> {
        if question.is_empty() {
            self.notice("[btw] Use: /btw <question>, asked on the side without stopping Kumi.");
            return Ok(());
        }
        // Its words come as `aside` events too, which plain mode leaves: the answer is said once it's all in.
        let answer = self.session().op(op::Aside { id: self.next_id(), question: question.into() }).await?;
        // A line each, as its lines came: a list stays a list.
        for line in answer.lines().filter(|line| !line.trim().is_empty()) {
            self.notice(&format!("[btw] {line}"));
        }
        Ok(())
    }
    /// /jobs: the background jobs, a line each, those still running first.
    fn jobs(&self) {
        let mut jobs = self.session().jobs();
        if jobs.is_empty() {
            self.notice("[jobs] No background jobs.");
            return;
        }
        jobs.sort_by_key(|job| enum_name(&job.state) != "running");
        for job in jobs {
            let title = head(&sanitize_text(&job.title, &self.0.options.secrets), 80);
            let doing = job
                .doing
                .as_deref()
                .map(|doing| format!(" · {}", head(&sanitize_text(doing, &self.0.options.secrets), 60)))
                .unwrap_or_default();
            self.notice(&format!("[jobs] Job {} · {title} · {}{doing}", job.id, enum_name(&job.state)));
        }
    }
    /// /slots [words]: which model does each listening job, and a swap of one, as the full-screen app says them. One swap
    /// at a time, since a model's fetch can take minutes, and Ctrl-C stops it.
    async fn slots(&self, words: &str) {
        if !self.session().capabilities().slots {
            self.notice("[slots] Kumi's listening isn't set up here.");
            return;
        }
        let swap = kumi_listening::slots::parse(words) != Ok(kumi_listening::slots::Asked::Show);
        if swap && self.0.state.borrow().slots.is_some() {
            self.notice("[slots] Kumi is still trying a model for /slots: wait for it, or Ctrl-C stops it.");
            return;
        }
        // What's under way comes as `slots-progress` events, with this id.
        let id = self.next_id();
        if swap {
            self.0.state.borrow_mut().slots = Some(id);
        }
        let said = self.session().op(op::Slots { id, words: words.into() }).await;
        {
            let mut state = self.0.state.borrow_mut();
            if state.slots == Some(id) {
                state.slots = None;
            }
        }
        match said {
            Ok(kumi_listening::slots::Said::Slots { lines, footer }) => {
                for line in lines {
                    self.notice(&format!("[slots] {line}"));
                }
                self.notice(&format!("[slots] {footer}"));
            }
            Ok(kumi_listening::slots::Said::Done(said) | kumi_listening::slots::Said::Refused(said)) => {
                self.notice(&format!("[slots] {said}"))
            }
            Err(error) => self.error(&error),
        }
    }
    /// /music: what Kumi learns your taste from, as the full-screen app lists it. Changing it (add, remove, pause, forget)
    /// is the full-screen app's.
    async fn music(&self, words: &str) {
        if !words.is_empty() {
            self.notice("[music] Adding, removing, pausing or forgetting your music is the full-screen app's; /music lists it here.");
            return;
        }
        match self.session().op(op::Music {}).await {
            Ok(view) => {
                for line in crate::tui::app::music_lines(&view.status, view.agreed, view.held, view.found.as_ref(), now_ms_f64()) {
                    self.notice(&format!("[music] {line}"));
                }
            }
            Err(error) => self.error(&error),
        }
    }
    /// Answers `verb`, a command plain mode has run since before the shared table (its own /pin, /unpin and /forget
    /// among them), or sends a turn.
    async fn command(&self, verb: &str, argument: Option<&str>, command: &str, input: &str) -> Result<(), RuntimeError> {
        let session = self.session();
        let can = session.capabilities();
        match verb{
 "/model"=>{self.model_command(argument).await?;},
 "/effort"=>{let choices=EFFORTS.map(|e|e.as_str()).join(", ");if let Some(argument)=argument{let effort=Effort::parse(argument);if argument!="default"&&effort.is_none(){self.notice(&format!("[effort] Choose one of {choices} or default."));}else{session.op(op::SetEffort{effort}).await?;self.notice(&format!("[effort] {argument}."));}}else{self.notice(&format!("[effort] {}. Choose one of {choices} or default.",self.current_model().and_then(|c|c.effort).map(|e|e.as_str()).unwrap_or("the model's default")));}},
 "/fast"=>{let view=session.models();let on=!view.as_ref().is_some_and(|v|v.fast);let current=view.map(|v|v.current);match session.op(op::SetFast{on}).await?{Some(tier)=>self.notice(&format!("[fast] {} is on{}.",tier.name,tier.description.map(|d|format!(": {d}")).unwrap_or_default())),None if on=>self.notice(&format!("[fast] {} has no faster tier.",current.and_then(|c|c.name.or(c.model)).unwrap_or_else(||"This model".into()))),None=>self.notice("[fast] Back to the standard tier."),}},
 "/memory"=>{let Some(memory)=session.op(op::Memory{}).await?else{self.notice("[memory] Kumi keeps no notes here.");return Ok(());};let list=|notes:&[kumi_agent::core::contracts::MemoryNote]|{if notes.is_empty(){"none".into()}else{notes.iter().map(|n|format!("{}{} {}",n.id,if n.pinned{" (pinned)"}else{""},n.text)).collect::<Vec<_>>().join(" · ")}};self.notice(&format!("[memory] About you: {}",list(&memory.memory.producer)));self.notice(&if memory.saved{format!("[memory] About {}: {}",memory.set_name.clone().unwrap_or_else(||format!("this {}",crate::daw::words().place)),list(&memory.memory.set))}else{format!("[memory] This {} isn't saved yet; notes about it are kept once it is.",crate::daw::words().place)});if can.techniques{let techniques=session.op(op::Techniques{}).await?;self.notice(&format!("[memory] Techniques: {}",if techniques.is_empty(){"none yet".into()}else{techniques.iter().map(|t|format!("{} {} (for {})",t.id,t.name,t.fits)).collect::<Vec<_>>().join(" · ")}));}if can.taste{let taste=session.op(op::Taste{}).await?;self.notice(&format!("[memory] From your {}: {}",crate::daw::words().places,if taste.is_empty(){"nothing yet".into()}else{taste.iter().enumerate().map(|(i,l)|format!("u{} {}",i+1,l.line)).collect::<Vec<_>>().join(" · ")}));self.0.state.borrow_mut().taste=taste;}},
 "/recipes"=>{let recipes=session.op(op::Recipes{}).await?;self.notice(&if recipes.is_empty(){format!("[recipes] None yet. Ask Kumi to save a way of working, or say \"watch me\" and do it in {}.",crate::daw::words().app)}else{format!("[recipes] {}",recipes.iter().map(|r|format!("{}{}: {}",r.name,if r.params.is_empty(){String::new()}else{format!(" (needs {})",r.params.iter().map(|p|p.name.as_str()).collect::<Vec<_>>().join(", "))},r.about)).collect::<Vec<_>>().join(" · "))});},
 "/resume"|"/conversations"|"/rename"|"/fork" if !can.conversations=>self.notice(&format!("[conversations] {}",crate::tui::app::not_kept())),
 "/resume"|"/conversations"=>{let words=string::trim(command.strip_prefix(verb).unwrap_or("")).to_string();self.resume_command(&words).await?;},
 "/rename"=>{let words=string::trim(command.strip_prefix(verb).unwrap_or("")).to_string();self.rename_command(&words).await?;},
 "/export"=>{let words=string::trim(command.strip_prefix(verb).unwrap_or("")).to_string();self.export_command(&words, false).await;},
 "/share"=>{self.export_command("html", true).await;},
 "/fork"=>{let name=string::trim(command.strip_prefix(verb).unwrap_or("")).to_string();if session.op(op::ForkConversation{name:(!name.is_empty()).then(||name.clone()),before:None}).await?.is_none(){self.notice("[fork] Nothing to fork yet: ask something first.");}},
 "/note"=>{let usage="[memory] Use: /note <id> <new words>, with an id from /memory.";let words=argument.and_then(|id|command.split_once(id)).map(|(_,words)|words.trim()).unwrap_or("");match argument.filter(|_|!words.is_empty()&&can.change_note){Some(id)=>match session.op(op::ChangeNote{id:id.into(),change:NoteChange::Text(words.into())}).await{Ok(Some(_))=>self.notice(&format!("[memory] Changed note {id}.")),Ok(None)=>self.notice(usage),Err(error)=>self.notice(&format!("[memory] {}",error.message()))},None=>self.notice(usage)}},
 "/pin"|"/unpin"=>{let pin=verb=="/pin";let usage=format!("[memory] Use: {verb} <id>, with a note's id from /memory.");match argument.filter(|_|can.change_note){Some(id)=>match session.op(op::ChangeNote{id:id.into(),change:NoteChange::Pinned(pin)}).await?{Some(_)=>self.notice(&if pin{format!("[memory] Pinned {id}: Kumi keeps it even when its memory is full.")}else{format!("[memory] Unpinned {id}.")}),None=>self.notice(&usage)},None=>self.notice(&usage)}},
 "/forget"=>{let invalid="[memory] Use: /forget <id>, with an id from /memory.";if argument.is_some_and(|a|a.starts_with('u'))&&can.forget_taste{if self.0.state.borrow().taste.is_empty(){let taste=session.op(op::Taste{}).await?;self.0.state.borrow_mut().taste=taste;}let n=number::parse(&argument.unwrap()[1..]).unwrap_or(f64::NAN)-1.;let line=if n>=0.&&n.fract()==0.{self.0.state.borrow().taste.get(n as usize).cloned()}else{None};if let Some(line)=line{if session.op(op::ForgetTaste{id:line.id.clone()}).await?{self.notice(&format!("[memory] Forgot, from your {}: {}",crate::daw::words().places,line.line));}else{self.notice(invalid);}}else{self.notice(invalid);}}else if argument.is_some_and(|a|a.starts_with('t'))&&can.forget_technique{if !session.op(op::ForgetTechnique{id:argument.unwrap().into()}).await?{self.notice(invalid);}}else if let Some(argument)=argument{if session.op(op::Forget{id:argument.into()}).await?.is_none(){self.notice(invalid);}}else{self.notice(invalid);}},
 "/willington"=>{match self.0.options.willington.clone().and_then(|w|(w.on)().map(|on|(w,on))){Some((willington,on))=>match (willington.set)(!on).await{Ok(said)=>{self.notice(&format!("[willington] {said}"));session.op(op::Reconfigure{}).await?;},Err(error)=>self.notice(&format!("[willington] Kumi couldn't turn Willington's bindings {}: {error}",if on{"off"}else{"on"})),},None=>self.notice("Unknown command. Use /help."),}},
 "/login"=>self.notice(&format!("[login] Sign in from a shell: {} login <provider> (openai-codex, anthropic, openai, opencode). The full-screen app signs in here.",*KUMI)),
 "/logout"=>{if let Some(provider)=argument.and_then(ProviderId::parse){self.notice(&if session.op(op::SignOut{provider}).await?{format!("[logout] Signed out of {}.",provider.as_str())}else{format!("[logout] There was no sign-in for {} to remove.",provider.as_str())});}else{self.notice(&format!("[logout] Use: /logout <provider> ({}).",table::rows().map(|row|row.id).collect::<Vec<_>>().join(", ")));}},
 _=>{if command=="/undo"{if let Some(change)=session.op(op::Undo{id:None}).await?{self.notice(&if enum_name(change.state)=="undone"{format!("[undo] Undid: {}",change.title)}else{string::trim(&format!("[undo] Kept: {}. {}",change.title,change.note.unwrap_or_default())).into()});}}else if command=="/refresh"{session.op(op::Refresh{}).await?;}else if command=="/reconnect"&&can.reconnect{session.op(op::Reconnect{}).await?;}else if verb=="/new"{self.notice("── New conversation. Kumi won't use what's above ──");session.op(op::NewConversation{}).await?;let name=string::trim(command.strip_prefix(verb).unwrap_or(""));if !name.is_empty(){if can.conversations{session.op(op::RenameConversation{id:None,name:Some(name.into())}).await?;}else{self.notice(&format!("[conversations] {}",crate::tui::app::not_kept()));}}}else if command_like(command){self.notice("Unknown command. Use /help.");}else if let Some(outcome)=self.instant(input).await{self.notice(&format!("[instant] {}",outcome.text));}else{self.turn(input).await?;}}
 }
        Ok(())
    }
    /// This Set's kept conversations, then other Sets' newest: one numbered list, the same each time it's asked for.
    async fn listed(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        let here = self.session().op(op::Conversations {}).await?;
        let others =
            self.session().op(op::AllConversations {}).await?.into_iter().filter(|row| !here.iter().any(|h| h.id == row.id)).take(10);
        Ok(here.iter().cloned().chain(others).collect())
    }
    /// /export [md | html | json], and /share (`again`: a web page, the one saved last while nothing changed): this
    /// conversation saved in the folder Kumi was started in.
    async fn export_command(&self, words: &str, again: bool) {
        let format = match words.to_ascii_lowercase().as_str() {
            "" | "md" | "markdown" => Format::Markdown,
            "html" | "page" | "web" => Format::Html,
            "json" | "kumi" => Format::Json,
            _ => {
                self.notice("[export] Use /export [md | html | json] (Markdown, a web page, or a file another Kumi imports).");
                return;
            }
        };
        let redact = crate::export::redactor_now(self.0.options.redact.as_ref(), &self.0.options.secrets).await;
        let found = match self.session().op(op::ConversationToExport {}).await {
            Ok(found) => found,
            Err(error) => {
                self.error(&error);
                return;
            }
        };
        let exported = match crate::export::exported(found, format, &*redact) {
            Ok(exported) => exported,
            Err(why) => {
                self.notice(&format!("[export] {why}"));
                return;
            }
        };
        // /share again with nothing changed: the page saved last, rather than another just like it.
        if format == Format::Html {
            let saved = self.0.state.borrow().shared.clone().filter(|(key, path)| *key == exported.key && path.exists());
            if let Some((_, path)) = saved.filter(|_| again) {
                self.notice(&format!("[export] This conversation's web page is saved already: {}.", path.display()));
                return;
            }
        }
        match crate::export::save_in(self.0.options.folder.as_deref(), &exported.text, &exported.name) {
            Ok(path) => {
                if format == Format::Html {
                    self.0.state.borrow_mut().shared = Some((exported.key, path.clone()));
                }
                self.notice(&format!("[export] {}", crate::export::saved_words(format, &path, &self.0.options.command)));
            }
            Err(why) => self.notice(&format!("[export] {why}")),
        }
    }
    /// /resume: the list; or the conversation a number from it, an id, the start of one or a name gives.
    async fn resume_command(&self, words: &str) -> Result<(), RuntimeError> {
        let kept = self.listed().await?;
        let mention = |row: &ConversationSummary, with_set: bool| {
            let title = head(&sanitize_text(&row.title(), &self.0.options.secrets), 80);
            let when = if row.current { "this one".into() } else { since(row.saved_at as f64, now_ms_f64()) };
            let set = row.set.as_deref().map(|set| head(&sanitize_text(set, &self.0.options.secrets), 40)).unwrap_or_else(|| {
                if row.place == kumi_types::place::IMPORTED {
                    "imported".into()
                } else {
                    format!("a {}", crate::daw::words().place)
                }
            });
            format!(
                "{} ({}{}{when}, {} {})",
                if title.is_empty() { "(nothing asked yet)" } else { &title },
                if with_set { format!("{set}, ") } else { String::new() },
                if row.pinned { "pinned, " } else { "" },
                row.turns,
                if row.turns == 1 { "request" } else { "requests" }
            )
        };
        if words.is_empty() {
            if kept.is_empty() {
                self.notice("[conversations] None kept yet: a conversation is kept once you've asked something.");
                return Ok(());
            }
            let here = self.session().op(op::Conversations {}).await?.len();
            let rows: Vec<String> = kept
                .iter()
                .enumerate()
                .map(|(at, row)| {
                    let other = if at == here && at > 0 {
                        format!("Other {}: ", crate::daw::capitalized(crate::daw::words().places))
                    } else {
                        String::new()
                    };
                    format!("{other}{}. {}", at + 1, mention(row, at >= here))
                })
                .collect();
            self.notice(&format!("[conversations] {}. /resume <number, id or name> goes back to one.", rows.join(" · ")));
            return Ok(());
        }
        let n = parse_int(words) - 1.;
        let found = if n >= 0. && n.fract() == 0. && words.bytes().all(|b| b.is_ascii_digit()) {
            match kept.get(n as usize) {
                Some(row) => ConversationMatch::One(row.clone()),
                None => {
                    self.notice("[conversations] Use: /resume <number>, with a number from /resume.");
                    return Ok(());
                }
            }
        } else {
            self.session().op(op::FindConversation { word: words.into() }).await?
        };
        match found {
            ConversationMatch::One(row) if row.current => self.notice("[conversations] That's this one."),
            ConversationMatch::One(row) => {
                if !self.session().op(op::ResumeConversation { id: row.id.clone() }).await? {
                    self.notice("[conversations] That conversation isn't kept any more.");
                }
            }
            ConversationMatch::Several(rows) => self.notice(&format!(
                "[conversations] Several fit: {}. /resume <id> goes back to one.",
                rows.iter().take(10).map(|row| format!("{} {}", row.id, mention(row, true))).collect::<Vec<_>>().join(" · ")
            )),
            ConversationMatch::Gone => self.notice("[conversations] That conversation isn't kept any more."),
            ConversationMatch::None => self.notice(&format!(
                "[conversations] No conversation {}; /resume lists them.",
                head(&sanitize_text(words, &self.0.options.secrets), 60)
            )),
        }
        Ok(())
    }
    /// /rename <name>, or (a kept one) /rename <id> <name>.
    async fn rename_command(&self, words: &str) -> Result<(), RuntimeError> {
        let session = self.session();
        if words.is_empty() {
            let title = session.op(op::ConversationNow {}).await.ok().flatten().map(|now| now.title).unwrap_or_default();
            self.notice(&format!("[rename] Use: /rename <name>. Now: {}", if title.is_empty() { "(no name yet)" } else { &title }));
            return Ok(());
        }
        let (first, rest) = words.split_once(' ').map(|(a, b)| (a, string::trim(b))).unwrap_or((words, ""));
        let kept = !rest.is_empty()
            && matches!(session.op(op::FindConversation { word: first.into() }).await?, ConversationMatch::One(row) if row.id == first);
        let (id, name) = if kept { (Some(first), rest) } else { (None, words) };
        self.notice(&if session.op(op::RenameConversation { id: id.map(str::to_string), name: Some(name.into()) }).await? {
            format!("[rename] Named “{}”.", head(&sanitize_text(name, &self.0.options.secrets), 60))
        } else {
            "[rename] That conversation isn't kept any more.".into()
        });
        Ok(())
    }
    /// A fixed phrase ("tempo 124", "mute drums") run at once with no model call; None sends it to the model.
    async fn instant(&self, input: &str) -> Option<kumi_agent::core::contracts::InstantOutcome> {
        if !self.0.options.instant || !self.session().capabilities().instant || kumi_agent::core::phrases::parse(input).is_none() {
            return None;
        }
        self.session().op(op::Instant { text: input.into() }).await.ok().flatten()
    }
    async fn model_command(&self, argument: Option<&str>) -> Result<(), RuntimeError> {
        let session = self.session();
        let Some(argument) = argument else {
            let current = self.current_model();
            let mut available = session
                .op(op::Providers {})
                .await?
                .into_iter()
                .filter(|p| p.signed_in)
                .map(|p| p.id.as_str().to_string())
                .collect::<Vec<_>>();
            available.extend(session.op(op::LocalModels {}).await?.into_iter().filter(|s| s.running).map(|s| s.id));
            self.notice(&format!(
                "[model] {}{}. List a provider's with /model <provider> ({}); choose with /model <provider>/<model>.",
                current.as_ref().and_then(|c| c.model.as_deref()).unwrap_or("none chosen"),
                current.as_ref().and_then(|c| c.effort).map(|e| format!(", effort {}", e.as_str())).unwrap_or_default(),
                if available.is_empty() { "sign in first, or open Ollama or LM Studio".into() } else { available.join(", ") }
            ));
            return Ok(());
        };
        if !argument.contains('/')
            && (ProviderId::parse(argument).is_some() || session.op(op::LocalModels {}).await?.iter().any(|s| s.id == argument))
        {
            let listed = session.op(op::Models { provider: argument.into(), refresh: false }).await?;
            self.notice(&format!(
                "[model] {argument}: {}",
                if listed.is_empty() {
                    "no models listed".into()
                } else {
                    listed.iter().map(|m| m.model.as_str()).collect::<Vec<_>>().join(", ")
                }
            ));
        } else {
            let note = session.op(op::ChooseModel { model: argument.into() }).await?;
            let fast = self
                .current_model()
                .and_then(|c| c.fast)
                .map(|tier| format!(" {} is on; /fast turns it off.", tier.name))
                .unwrap_or_default();
            self.notice(&format!(
                "[model] {argument} from the next answer on.{}{fast}",
                note.filter(|n| !n.is_empty()).map(|n| format!(" {n}")).unwrap_or_default()
            ));
        }
        Ok(())
    }
}
fn parse_int(value: &str) -> f64 {
    let value = string::trim(value);
    let sign = if value.starts_with('-') { -1. } else { 1. };
    let value = value.strip_prefix(['+', '-']).unwrap_or(value);
    let digits = value.chars().take_while(char::is_ascii_digit).collect::<String>();
    sign * digits.parse::<f64>().unwrap_or(f64::NAN)
}
impl Terminal for PlainTerminal {
    fn run(&self) -> LocalBoxFuture<'static, i32> {
        {
            let mut state = self.0.state.borrow_mut();
            if state.started || state.closing {
                return self.done();
            }
            state.started = true;
            state.was_raw = self.0.options.input.is_raw();
            let tty = self.0.options.input.is_tty() && self.0.options.output.is_tty();
            let history = self
                .0
                .options
                .history
                .as_ref()
                .map(|h| h.borrow().entries().iter().rev().map(|s| s.replace('\n', " ")).collect())
                .unwrap_or_default();
            state.presentation = Some(Presentation::new(tty, history));
        }
        let tty = self.0.options.input.is_tty() && self.0.options.output.is_tty();
        let active: Rc<dyn TerminalInput> = if tty {
            let keys = Rc::new(KeyInput::new(self.0.options.input.clone()));
            *self.0.keys.borrow_mut() = Some(keys.clone());
            let weak = Rc::downgrade(&self.0);
            *self.0.parser.borrow_mut() = Some(InputParser::new(Rc::new(move |event| {
                if let Some(inner) = weak.upgrade() {
                    Self(inner).typed(event);
                }
            })));
            let _ = keys.set_raw_mode(true);
            keys
        } else {
            self.0.options.input.clone()
        };
        let weak = Rc::downgrade(&self.0);
        active.on_end(Rc::new(move || {
            if let Some(inner) = weak.upgrade() {
                let this = Self(inner);
                let last = {
                    let mut state = this.0.state.borrow_mut();
                    let tail = state.decoder.end();
                    state.pipe_buffer.push_str(&tail);
                    std::mem::take(&mut state.pipe_buffer)
                };
                if !last.is_empty() {
                    this.spawn_submit(last);
                }
                drop(this.finish(0));
            }
        }));
        let weak = Rc::downgrade(&self.0);
        self.0.options.input.on_error(Rc::new(move |_| {
            if let Some(inner) = weak.upgrade() {
                drop(Self(inner).finish(1));
            }
        }));
        let weak = Rc::downgrade(&self.0);
        self.0.options.output.on_error(Rc::new(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.state.borrow_mut().output_failed = true;
                drop(Self(inner).finish(1));
            }
        }));
        self.notice(&format!(
            "Kumi · {} · {}",
            self.current_model().and_then(|current| current.model).as_deref().unwrap_or("no model chosen yet"),
            if self.0.options.mode == "inference-only" {
                format!("MCP disconnected / No {} access", crate::daw::words().app)
            } else {
                format!("MCP connecting / {} unverified", crate::daw::words().app)
            }
        ));
        self.notice(&format!(
            "Each {0}'s conversations are kept: its latest continues next time, and /resume goes back to earlier ones (any {0}'s, by number, id or name). /help for commands.",
            crate::daw::words().place
        ));
        if let Some(notice) = self.0.options.startup_notice.as_ref().filter(|s| !s.is_empty()) {
            self.notice(notice);
        }
        if let Some(news) = &self.0.options.whats_new {
            self.news(news, crate::whats_new::more_line(news));
        }
        if self.0.options.willington.as_ref().is_some_and(|w| (w.on)() == Some(false)) {
            if let Some(words) = crate::daw::current().toggle_words() {
                self.notice(&format!("[{}] {}", words.tag, words.off_at_start));
            }
        }
        let weak = Rc::downgrade(&self.0);
        active.resume(Rc::new(move |bytes| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).bytes(bytes);
            }
        }));
        if self.current_model().is_some_and(|current| current.model.is_none()) {
            let this = self.clone();
            tokio::task::spawn_local(async move {
                if let Ok(chosen) = this.session().op(op::ChooseDefault {}).await {
                    if !this.0.state.borrow().closing {
                        this.notice(&if let Some(chosen)=chosen{if let Some(where_)=chosen.model.r#where.filter(|s|!s.is_empty()){format!("[model] {}, in {} {where_}. /model changes it.{}",chosen.model.id,this.session().provider_name(&chosen.model.provider),chosen.note.filter(|s|!s.is_empty()).map(|s|format!(" {s}")).unwrap_or_default())}else{format!("[model] {}, the first {} lists. /model changes it.",chosen.model.id,chosen.model.provider)}}else{format!("[model] Not signed in to a provider yet. Sign in with: {} login <provider>, then /model; or open Ollama or LM Studio.",*KUMI)});
                    }
                }
            });
        } else if self.current_model().is_some() {
            // A saved model the provider's own list no longer shows: said once, with how to choose another.
            let this = self.clone();
            tokio::task::spawn_local(async move {
                if let Some(said) = this.session().op(op::MissingModel {}).await.ok().flatten() {
                    if !this.0.state.borrow().closing {
                        let provider = this.current_model().and_then(|current| current.provider).unwrap_or_default();
                        this.notice(&format!(
                            "[model] {said}. /model {provider} lists the ones it does; /model {provider}/<model> chooses one."
                        ));
                    }
                }
            });
        }
        let this = self.clone();
        tokio::task::spawn_local(async move {
            if !this.0.state.borrow().closing {
                if let Err(error) = this.session().op(op::Start {}).await {
                    if !this.0.state.borrow().closing {
                        this.error(&error);
                        this.finish(1).await;
                    }
                    return;
                }
                // `kumi -r`: the kept conversations to choose from (those the words fit, given words).
                if let Some(words) = this.0.options.choose_conversation.clone() {
                    if let Err(error) = this.resume_command(&words).await {
                        this.error(&error);
                    }
                    this.prompt();
                }
            }
        });
        self.done()
    }
    fn handle_event(&self, event: SessionEvent) {
        if self.0.state.borrow().closing {
            return;
        }
        let value = serde_json::to_value(&event).expect("session event serializes");
        let get = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or("");
        let n = |key: &str| num(&value[key]);
        match get("type") {
            "state" => {
                let mut state = self.0.state.borrow_mut();
                let queued = if get("state") == "idle" && !state.answering { state.queued.take() } else { None };
                if get("state") == "running" {
                    state.turns_begun += 1;
                    state.suppress = false;
                    state.displayed_bytes = 0;
                    state.text.discard();
                    state.started_at = perf_now();
                    state.first_text_ms = None;
                }
                if get("state") == "cancelling" {
                    state.suppress = true;
                    state.text.discard();
                }
                let carry = (get("state") == "idle" && queued.is_none()).then(|| state.carry.first().copied()).flatten();
                drop(state);
                if let Some(line) = queued {
                    let this = self.clone();
                    tokio::task::spawn_local(async move {
                        this.spawn_submit(line);
                    });
                } else if let Some(id) = carry {
                    self.carry_on(id);
                }
            }
            // A /goal as its event says it, kept for /goal; a sound match taking over clears it, as the full-screen app's
            // GOAL tab shows the match then.
            "objective" => {
                if let SessionEvent::Objective(status) = &event {
                    self.0.state.borrow_mut().objective = Some(status.clone());
                }
            }
            "objective-cleared" => self.0.state.borrow_mut().objective = None,
            // A /slots fetch or check under way, as it goes.
            "slots-progress" => {
                if let SessionEvent::SlotsProgress { line, .. } = &event {
                    self.notice(&format!("[slots] {line}"));
                }
            }
            "goal" => {
                if let SessionEvent::Goal(status) = &event {
                    let mut state = self.0.state.borrow_mut();
                    state.matching = matches!(status.state, GoalPhase::Running | GoalPhase::Starting);
                    state.objective = None;
                }
            }
            "job" => {
                if let SessionEvent::Job(status) = &event {
                    use kumi_agent::core::jobs::{JobState, WhenDone};
                    let line = status.line.clone().map(|line| format!(": {line}")).unwrap_or_default();
                    match status.state {
                        JobState::Running if status.steps == 0 && status.tool.is_none() && status.doing.as_deref() == Some("starting") => {
                            self.notice(&format!("[job {}] started in the background: {}", status.id, status.title))
                        }
                        JobState::Running | JobState::Interrupted => {}
                        state => {
                            let how = match state {
                                JobState::Done => "done",
                                JobState::Stopped => "stopped",
                                _ => "failed",
                            };
                            self.notice(&format!("[job {}] {}: {how}{line}", status.id, status.title));
                            if status.when_done == WhenDone::Continue && state != JobState::Stopped {
                                self.0.state.borrow_mut().carry.push(status.id);
                                if self.session().status().state == TurnState::Idle {
                                    self.carry_on(status.id);
                                }
                            }
                        }
                    }
                }
            }
            "carry-on" => self.notice(&format!("[job {}] Kumi carries on with its result", n("job"))),
            "connection" => self.notice(&format!(
                "[connection] MCP/{}: {}{}",
                crate::daw::words().app,
                get("state"),
                if get("state") != "connected" {
                    format!("; no verified current {} observation", crate::daw::words().app)
                } else {
                    String::new()
                }
            )),
            "observation" => self.notice(&format!("[observation] {}", get("label"))),
            // A busy DAW (a dialog is open) is said as it starts and ends; Live's focus never says it.
            "focus" => {
                let busy = value["focus"]["busy"].as_str().map(str::to_owned);
                let before = std::mem::replace(&mut self.0.state.borrow_mut().busy, busy.clone());
                let app = crate::daw::words().app;
                match (before, busy) {
                    (None, Some(why)) => self.notice(&format!("[connection] MCP/{app}: busy ({why})")),
                    (Some(_), None) => self.notice(&format!("[connection] MCP/{app}: answering again")),
                    _ => {}
                }
            }
            "library" => {
                let mut state = self.0.state.borrow_mut();
                if !state.told_library
                    && matches!(value["status"]["state"].as_str(), Some("learning" | "paused"))
                    && value["status"]["learnedAt"].as_f64().unwrap_or(0.) == 0.
                {
                    state.told_library = true;
                    drop(state);
                    self.notice("Learning your library in the background…");
                }
            }
            "music" => {
                let mut state = self.0.state.borrow_mut();
                // The first time Kumi learns their music: nothing heard yet.
                let heard = value["status"]["light"].as_u64().unwrap_or(0) + value["status"]["full"].as_u64().unwrap_or(0);
                if !state.told_music
                    && value["status"]["state"].as_str() == Some("learning")
                    && heard == 0
                    && value["status"]["learnedAt"].as_f64().unwrap_or(0.) == 0.
                {
                    state.told_music = true;
                    drop(state);
                    self.notice("Learning your music in the background…");
                }
            }
            "resumed" => {
                let when = since(value["savedAt"].as_f64().unwrap_or(0.), now_ms_f64());
                let chosen = value["chosen"].as_bool() == Some(true);
                let unreadable = value["unreadable"].as_bool() == Some(true);
                self.notice(&if chosen {
                    format!("── Back to your conversation from {when} ──")
                } else if unreadable {
                    format!("[resumed] Your conversation from {when}, which this model can't continue:")
                } else {
                    format!("[resumed] Continuing your conversation from {when} (/new starts fresh).")
                });
                if chosen || unreadable {
                    let lines = array(&value["lines"]);
                    for line in &lines[lines.len().saturating_sub(6)..] {
                        self.notice(&format!(
                            "{}> {}",
                            if line["role"] == "user" { "you" } else { "kumi" },
                            head(&sanitize_text(str_(&line["text"]), &self.0.options.secrets).replace('\n', " "), 200)
                        ));
                    }
                }
            }
            "set-asked" => {
                let ask: Option<SetAsk> = serde_json::from_value(value["ask"].clone()).ok();
                self.0.state.borrow_mut().asking = None;
                if let Some(ask) = ask {
                    let clean = |text: &str| head(&sanitize_text(text, &self.0.options.secrets), 40);
                    let theirs = ask.set.as_deref().map(|set| format!("“{}”", clean(set))).unwrap_or_else(|| format!("its {}", ask.noun));
                    let open = ask
                        .open
                        .as_deref()
                        .map(|set| format!("“{}”", clean(set)))
                        .unwrap_or_else(|| format!("a {} not saved yet", ask.noun));
                    let title = if ask.title.is_empty() { "This conversation".into() } else { format!("“{}”", clean(&ask.title)) };
                    let first = if ask.by_hand {
                        format!("Open {theirs} in {daw} yourself (this conversation carries on once it's open)", daw = ask.daw)
                    } else {
                        format!("Open {theirs} in {daw} ({daw} may ask whether to save {open} first)", daw = ask.daw)
                    };
                    let question = format!(
                        "[conversations] {title} is about {theirs}, and {daw} has {open} open. 1. {first} · 2. Carry it on here, as a copy · 3. Cancel. Type 1, 2 or 3.",
                        daw = ask.daw
                    );
                    self.0.state.borrow_mut().asking = Some(question.clone());
                    self.notice(&question);
                }
            }
            "resend" => {
                let mut state = self.0.state.borrow_mut();
                if let Some(p) = state.presentation.as_mut().filter(|p| p.tty && p.editor.is_empty()) {
                    p.editor.insert(&get("text").replace('\n', " "));
                    p.redraw(self.0.options.output.as_ref());
                }
            }
            "catch-up" => {
                let catch = &value["catchUp"];
                let when = since(catch["lastSeenAt"].as_f64().unwrap_or(0.), now_ms_f64());
                let lines = array(&catch["lines"]);
                self.notice(&if lines.is_empty() {
                    format!("[since last time · {when}] Nothing changed.")
                } else {
                    format!(
                        "[since last time · {when}] {}{}",
                        lines.iter().map(str_).collect::<Vec<_>>().join("; "),
                        if catch["more"].as_f64().unwrap_or(0.) != 0. {
                            format!("; and {} more", num(&catch["more"]))
                        } else {
                            String::new()
                        }
                    )
                });
            }
            "change" => {
                let c = &value["change"];
                let (id, state) = (str_(&c["id"]).to_owned(), str_(&c["state"]).to_owned());
                if !id.is_empty() && self.0.state.borrow_mut().said.insert(id, state.clone()).as_deref() == Some(state.as_str()) {
                    return;
                }
                let title = str_(&c["title"]);
                let note = str_(&c["note"]);
                let text = match str_(&c["state"]) {
                    "applied" => Some(format!("[change] {title} (/undo takes it back)")),
                    "unsure" => {
                        let app = crate::daw::words().app;
                        Some(format!(
                            "[change] Check {app}: {title}. {}",
                            c["note"].as_str().map_or(format!("{app} didn't confirm it."), str::to_owned)
                        ))
                    }
                    "kept" => Some(string::trim(&format!("[change] Kept: {title}. {note}")).into()),
                    // Those that end together for one reason (all of a Set's, when Live opens another): the first as it
                    // comes, the rest in one line after it.
                    "expired" => {
                        let (first, earlier) = {
                            let mut state = self.0.state.borrow_mut();
                            match state.expired.as_mut() {
                                Some((reason, rest)) if reason == note => {
                                    rest.push(title.to_owned());
                                    (false, None)
                                }
                                _ => (true, state.expired.replace((note.to_owned(), vec![]))),
                            }
                        };
                        if let Some(line) = earlier.and_then(|(reason, rest)| expired_rest(&reason, &rest)) {
                            self.notice(&line);
                        }
                        if first {
                            let this = self.clone();
                            let reason = note.to_owned();
                            tokio::task::spawn_local(async move {
                                tokio::task::yield_now().await;
                                let ended = {
                                    let mut state = this.0.state.borrow_mut();
                                    match state.expired.as_ref() {
                                        Some((current, _)) if *current == reason => state.expired.take(),
                                        _ => None,
                                    }
                                };
                                if let Some(line) = ended.and_then(|(reason, rest)| expired_rest(&reason, &rest)) {
                                    this.notice(&line);
                                }
                            });
                            Some(string::trim(&format!("[change] No undo anymore: {title}. {note}")).into())
                        } else {
                            None
                        }
                    }
                    "heard" => Some(format!("[heard] {title}{}", c.get("score").map(|n| format!(" · {}%", num(n))).unwrap_or_default())),
                    _ => None,
                };
                if let Some(text) = text {
                    self.notice(&text);
                }
            }
            "notice" => self.notice(get("message")),
            // The judge's rounds, a line each, so a loop's work shows here too.
            "judged" => {
                if let SessionEvent::Judged(round) = &event {
                    for line in round.lines() {
                        self.notice(&format!("[judged] {line}"));
                    }
                }
            }
            "error" => {
                self.error(&RuntimeError::plain(get("message")));
                self.0.state.borrow_mut().text.discard();
                if get("kind") == "auth" && self.session().cloud_provider(get("provider")).is_some() {
                    self.notice(&format!("[login] Sign in from a shell: {} login {}", *KUMI, get("provider")));
                }
            }
            "text" => {
                let mut state = self.0.state.borrow_mut();
                if state.suppress {
                    return;
                }
                if state.first_text_ms.is_none() {
                    state.first_text_ms = Some(number::round(perf_now() - state.started_at));
                }
                state.displayed_bytes += get("text").len();
                if state.displayed_bytes > 256 * 1024 {
                    drop(state);
                    self.notice("Assistant output exceeded the terminal bound; cancelling.");
                    self.interrupt();
                    return;
                }
                let text = state.text.push(get("text"));
                if let Some(p) = &mut state.presentation {
                    p.text(self.0.options.output.as_ref(), &text);
                }
            }
            "tool-start" => {
                if !self.0.state.borrow().suppress {
                    self.notice(&format!("[tool] {} started", get("name")));
                }
            }
            "retry" => {
                if !self.0.state.borrow().suppress {
                    let wait = value["waitMs"].as_f64().unwrap_or(0.);
                    self.notice(&if value["wait"] == "limit" {
                        let at = kumi_common::time::now_ms_f64() + wait;
                        format!("[wait] {}; carrying on at {} (Ctrl-C stops)", get("reason"), kumi_ai::failure::reset_words(at, at - wait))
                    } else if value["wait"] == "offline" {
                        // A certificate or a proxy in the way names itself in the reason.
                        let offline = if value["cause"].is_string() { "" } else { "offline: " };
                        format!("[wait] {offline}{}; trying again in {} s", get("reason"), (wait / 1000.).ceil().max(1.))
                    } else if wait > 0. {
                        format!("[wait] {}; trying again in {} s", get("reason"), (wait / 1000.).ceil().max(1.))
                    } else {
                        format!("[wait] {}; carrying on", get("reason"))
                    });
                }
            }
            "model-switch" => {
                let model = get("model");
                self.notice(&if value["back"] == true {
                    format!("[model] {}: back to {model}", get("reason"))
                } else {
                    format!("[model] {}; carrying on with {model}", get("reason"))
                });
            }
            "tool-end" => {
                if !self.0.state.borrow().suppress {
                    self.notice(&format!(
                        "[tool] {} {} · {} ms",
                        get("name"),
                        if value["isError"] == true { "error" } else { "success" },
                        n("elapsedMs")
                    ));
                }
            }
            "remembered" => self.notice(&format!(
                "[memory] {} {}: {}{}",
                if value.get("replaced").is_some_and(|v| !v.is_null()) { "Updated a note" } else { "Noted" },
                if get("scope") == "producer" { "about you".to_string() } else { format!("about this {}", crate::daw::words().place) },
                str_(&value["note"]["text"]),
                if value["pending"] == true { format!(" (kept once the {} is saved)", crate::daw::words().place) } else { String::new() }
            )),
            "forgot" => self.notice(&format!("[memory] Forgot: {}", str_(&value["note"]["text"]))),
            // (tagged with the DAW's name as /daw takes it: [live] in Live, as always)
            "action" => self.notice(&format!("[{}] {}", crate::daw::command_name(crate::daw::current().id()), get("title"))),
            "watching" => self.notice(&if value["on"] == true {
                let words = crate::daw::words();
                format!(
                    "[{}] Kumi is watching the {}; do it in {}, then tell Kumi you're done.",
                    crate::daw::command_name(crate::daw::current().id()),
                    words.place,
                    words.app
                )
            } else {
                format!("[{}] Kumi stopped watching.", crate::daw::command_name(crate::daw::current().id()))
            }),
            "recipe" => self.notice(&format!(
                "[recipe] {} “{}” ({} steps)",
                match get("action") {
                    "running" => "Running",
                    "forgotten" => "Forgot",
                    "updated" => "Updated",
                    _ => "Saved",
                },
                get("name"),
                n("steps")
            )),
            "technique" if get("action") == "offered" => self.notice(&format!(
                "[technique] Keep “{}” as a technique? Say “keep the technique” in your next message to keep it.",
                str_(&value["technique"]["name"])
            )),
            "technique" => {
                let t = &value["technique"];
                self.notice(&format!(
                    "[technique] {} “{}”{}",
                    match get("action") {
                        "kept" => "Kept",
                        "updated" => "Updated",
                        "used" => "Using",
                        _ => "Forgot",
                    },
                    str_(&t["name"]),
                    if matches!(get("action"), "kept" | "updated") {
                        format!(" (for {}; /forget {} drops it)", str_(&t["fits"]), str_(&t["id"]))
                    } else {
                        String::new()
                    }
                ));
            }
            "lesson" => {
                self.notice(&format!("[{}] {}", if get("action") == "forgot" { "forgot a lesson" } else { "learned" }, get("line")))
            }
            "match" => {
                if get("state") == "done" && value.get("best").is_some() {
                    self.notice(&format!(
                        "[matching] {}{}% ({}), stopped: {}",
                        value.get("first").map(|v| format!("{}% → ", num(v))).unwrap_or_default(),
                        num(&value["best"]["score"]),
                        str_(&value["best"]["label"]),
                        value["stop"].as_str().unwrap_or("done")
                    ));
                }
            }
            "auditioned" => {
                let best = value.get("best");
                self.notice(&format!(
                    "[round {}] {}",
                    n("round"),
                    if let Some(best) = best {
                        format!(
                            "{}{}%{}",
                            value.get("previous").map(|v| format!("{}% → ", num(v))).unwrap_or_default(),
                            num(&best["score"]),
                            if array(&value["gaps"]).is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", array(&value["gaps"]).iter().map(str_).collect::<Vec<_>>().join(", "))
                            }
                        )
                    } else {
                        "listened".into()
                    }
                ));
            }
            "heard" => {
                self.notice(&if let Some(c) = value.get("compared") {
                    let headlines = array(&c["headlines"]);
                    format!(
                        "[heard] {} against {}: {}",
                        get("file"),
                        str_(&c["reference"]),
                        if headlines.is_empty() { "close".into() } else { headlines.iter().map(str_).collect::<Vec<_>>().join("; ") }
                    )
                } else {
                    format!("[heard] {} · {}", get("file"), get("summary"))
                });
            }
            "watched" => {
                let at = |value: &Value| {
                    let seconds = value.as_f64().unwrap_or(0.);
                    format!("{}:{:02}", (seconds / 60.).floor() as i64, (seconds % 60.).floor() as i64)
                };
                let words = match get("words") {
                    "captions" => "its captions",
                    "automatic" => "its automatic captions",
                    "transcribed" => "its speech, transcribed",
                    _ => "no words",
                };
                self.notice(&format!(
                    "[watched] “{}”{}: {}–{}, {words}{}{}",
                    head(get("title"), 120),
                    if value["duration"].as_f64().unwrap_or(0.) != 0. { format!(" ({})", at(&value["duration"])) } else { String::new() },
                    at(&value["from"]),
                    at(&value["to"]),
                    if array(&value["frames"]).is_empty() {
                        String::new()
                    } else {
                        format!(", frames at {}", array(&value["frames"]).iter().map(|f| at(&f["at"])).collect::<Vec<_>>().join(", "))
                    },
                    value.get("sound").map(|s| format!(", the sound at {}–{}", at(&s["from"]), at(&s["to"]))).unwrap_or_default()
                ));
                for note in array(&value["notes"]).iter().take(3) {
                    self.notice(&format!("[watched] {}", str_(note)));
                }
            }
            "web" => {
                if let SessionEvent::Web(event) = event {
                    let words = web_words(&event, &|text, max| {
                        let text = sanitize_text(text, &self.0.options.secrets);
                        head(
                            &text
                                .split(|c: char| c.is_whitespace() || c == '\u{feff}')
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                                .join(" "),
                            max,
                        )
                    });
                    self.notice(&format!(
                        "[web] {} {}{}",
                        words.lead,
                        words.title,
                        if words.detail.is_empty() { String::new() } else { format!(" · {}", words.detail) }
                    ));
                }
            }
            "turn-complete" => {
                let result = &value["result"];
                let reason = str_(&result["stopReason"]);
                let first;
                {
                    let mut state = self.0.state.borrow_mut();
                    if reason != "cancelled" && !state.suppress {
                        let text = state.text.finish();
                        if let Some(p) = &mut state.presentation {
                            p.text(self.0.options.output.as_ref(), &text);
                        }
                    } else {
                        state.text.discard();
                    }
                    first = state.first_text_ms;
                }
                let usage = result.get("usage");
                self.notice(&format!(
                    "[{reason}] first text {}; total {} ms; {}; cost unavailable",
                    first.map(|ms| format!("{} ms", number::to_string(ms))).unwrap_or("unavailable".into()),
                    n("elapsedMs"),
                    if let Some(usage) = usage {
                        format!(
                            "reported tokens in/out {}/{}; cache read/write {}/{}{}",
                            num(&usage["inputTokens"]),
                            num(&usage["outputTokens"]),
                            num(&usage["cacheReadTokens"]),
                            num(&usage["cacheWriteTokens"]),
                            if reason == "cancelled" { " (partial before cancellation)" } else { "" }
                        )
                    } else {
                        "usage unavailable".into()
                    }
                ));
            }
            _ => {}
        }
    }
    fn interrupt(&self) {
        let mut state = self.0.state.borrow_mut();
        if state.closing {
            return;
        }
        // A /slots swap under way stops first, as Esc stops it in the full-screen app.
        if let Some(id) = state.slots.take() {
            drop(state);
            self.session().tell(op::StopSlots { id }.into());
            self.notice("[slots] Stopping the model Kumi was trying.");
            return;
        }
        if !self.busy() {
            drop(state);
            drop(self.finish(0));
            return;
        }
        if state.cancelling {
            return;
        }
        state.cancelling = true;
        state.suppress = true;
        state.text.discard();
        drop(state);
        self.notice("[cancelling] Cancelling current work.");
        let this = self.clone();
        tokio::task::spawn_local(async move {
            if let Err(error) = this.session().op(op::Cancel {}).await {
                this.error(&error);
            }
            this.0.state.borrow_mut().cancelling = false;
        });
    }
    fn offer_update(&self, latest: &str) {
        let mut state = self.0.state.borrow_mut();
        if state.closing || state.newer.as_deref() == Some(latest) {
            return;
        }
        state.newer = Some(latest.into());
        drop(state);
        self.notice(&format!(
            "[update] Kumi {latest} is out (this is {}). /update gets it.",
            self.0.options.updates.as_ref().map(|u| u.current.as_str()).unwrap_or("an older one")
        ));
    }
    fn close(&self) -> LocalBoxFuture<'static, i32> {
        self.finish(0)
    }
    fn follow_daw(&self, daw: kumi_types::daw::DawId) -> bool {
        let Some(follow) = self.0.options.follow_daw.clone() else { return false };
        let waiting = {
            let state = self.0.state.borrow();
            state.closing || state.asking.is_some()
        };
        if waiting || self.busy() {
            return false;
        }
        let app = crate::daw::provider(daw).map(|provider| provider.profile().words.app).unwrap_or_default();
        self.notice(&format!("[daw] {app} is open, so Kumi opens again with it."));
        follow(daw);
        drop(self.finish(0));
        true
    }
}
fn str_(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
fn num(value: &Value) -> String {
    value.as_f64().map(number::to_string).unwrap_or("undefined".into())
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The line for the changes whose undo ended for one reason after the first, which was said already: how many,
/// with the first few by title.
fn expired_rest(reason: &str, rest: &[String]) -> Option<String> {
    if rest.is_empty() {
        return None;
    }
    let shown: Vec<&str> = rest.iter().take(3).map(String::as_str).collect();
    let more = rest.len() - shown.len();
    Some(
        string::trim(&format!(
            "[change] No undo anymore for {} more {}: {}{}. {reason}",
            rest.len(),
            if rest.len() == 1 { "change" } else { "changes" },
            shown.join("; "),
            if more > 0 { format!("; and {more} more") } else { String::new() }
        ))
        .into(),
    )
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    #[test]
    fn plain_mode_runs_or_declines_every_command_in_the_shared_table() {
        let now = all_on();
        let mut declined = vec![];
        for name in registry::COMMANDS.iter().flat_map(|command| std::iter::once(&command.name).chain(command.aliases)) {
            let effect = registry::run(name, &now);
            // A command the table has, by any of its names, comes to an effect plain mode's match answers: it runs it, or
            // says it's the full-screen app's. Never "Unknown command", and never a message for the model.
            assert!(!matches!(effect, Effect::Unknown { .. } | Effect::Message), "{name} isn't answered: {effect:?}");
            if full_screen_only(&effect) {
                declined.push(*name);
            }
        }
        declined.sort();
        assert_eq!(
            declined,
            ["/copy", "/map", "/palette", "/recipe", "/takeback", "/voice"],
            "what plain mode leaves to the full-screen app"
        );
    }

    #[test]
    fn plain_modes_own_commands_stay_out_of_the_shared_table_and_a_lone_slash_is_no_command_there() {
        for own in ["/pin", "/unpin", "/forget"] {
            assert!(registry::named(own).is_none(), "{own} is plain mode's own");
        }
        // run() takes a lone `/` as words; plain mode catches it first (#629's rule).
        assert_eq!(registry::run("/", &all_on()), Effect::Message);
        assert!(command_like("/") && command_like("/help") && !command_like("/Users/me/Music/kick.wav make it punchier"));
    }
}
