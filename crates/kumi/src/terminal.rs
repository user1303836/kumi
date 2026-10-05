//! The plain terminal, for pipes and KUMI_UI=plain, with a native line editor.
use crate::{
    config::safe_error_message,
    history::InputHistory,
    input::{KeyInput, TerminalInput, Utf8Decoder},
    models::ModelController,
    text::{library_line, sanitize_text, web_words, StreamingText},
    tui::{
        editor::Editor,
        keys::{InputEvent, InputParser},
        tty::TtyOutput,
        width::{cell_width, graphemes},
    },
    update::UpdateControl,
};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::{
    js::{number, string},
    time::{now_ms_f64, perf_now},
};
use kumi_runtime::{
    core::{
        contracts::{KeptLine, NoteChange, SessionController, SessionEvent, TurnState},
        errors::RuntimeError,
    },
    integrations::ableton::project::since,
    providers::{Effort, ProviderId, EFFORTS, PROVIDERS},
    KUMI,
};
use serde_json::Value;
use std::{cell::RefCell, rc::Rc, time::Duration};
pub trait Terminal {
    fn run(&self) -> LocalBoxFuture<'static, i32>;
    fn handle_event(&self, event: SessionEvent);
    fn offer_update(&self, latest: &str);
    fn interrupt(&self);
    fn close(&self) -> LocalBoxFuture<'static, i32>;
}
#[derive(Clone)]
pub struct TerminalOptions {
    pub controller: Rc<dyn SessionController>,
    pub input: Rc<dyn TerminalInput>,
    pub output: Rc<dyn TtyOutput>,
    pub models: Rc<dyn ModelController>,
    pub mode: String,
    pub startup_notice: Option<String>,
    pub secrets: Vec<String>,
    pub close_timeout_ms: Option<u64>,
    pub history: Option<Rc<RefCell<InputHistory>>>,
    pub updates: Option<UpdateControl>,
}
impl TerminalOptions {
    pub fn new(
        controller: Rc<dyn SessionController>,
        input: Rc<dyn TerminalInput>,
        output: Rc<dyn TtyOutput>,
        models: Rc<dyn ModelController>,
        mode: &str,
    ) -> Self {
        Self {
            controller,
            input,
            output,
            models,
            mode: mode.into(),
            startup_notice: None,
            secrets: vec![],
            close_timeout_ms: None,
            history: None,
            updates: None,
        }
    }
}
fn help() -> String {
    format!("/help · /status · /undo · /stop · /refresh · /reconnect (connect to Live again, keeping the conversation) · /new (forget this conversation and start fresh) · /conversations [number] (list this Set's, or go back to one) · /model [provider/model] · /effort [level|default] · /logout <provider> · /memory · /forget <id> · /note <id> <new words> · /pin <id> · /unpin <id> · /recipes · /update (get the newest Kumi) · /quit | Ctrl-C: cancel work; idle: exit. EOF exits. Sign in with: {} login <provider>.",*KUMI)
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
    started: bool,
    closing: bool,
    cancelling: bool,
    newer: Option<String>,
    taste: Vec<KeptLine>,
    told_library: bool,
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
}
struct Inner {
    options: TerminalOptions,
    state: RefCell<State>,
    parser: RefCell<Option<InputParser>>,
    keys: RefCell<Option<Rc<KeyInput>>>,
    done: tokio::sync::watch::Sender<Option<i32>>,
}
#[derive(Clone)]
pub struct PlainTerminal(Rc<Inner>);
pub fn create_terminal(options: TerminalOptions) -> PlainTerminal {
    let text = StreamingText::new(&options.secrets);
    PlainTerminal(Rc::new(Inner {
        options,
        state: RefCell::new(State {
            started: false,
            closing: false,
            cancelling: false,
            newer: None,
            taste: vec![],
            told_library: false,
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
        }),
        parser: RefCell::new(None),
        keys: RefCell::new(None),
        done: tokio::sync::watch::channel(None).0,
    }))
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
    fn notice(&self, message: &str) {
        let mut state = self.0.state.borrow_mut();
        if state.output_failed {
            return;
        }
        let text = head(&sanitize_text(message, &self.0.options.secrets).replace('\n', " "), 2048);
        if let Some(p) = &mut state.presentation {
            p.notice(self.0.options.output.as_ref(), &text, false);
        }
    }
    fn error(&self, error: &RuntimeError) {
        self.notice(&format!("[error] {}", safe_error_message(Some(&error.message()), &self.0.options.secrets)));
    }
    fn busy(&self) -> bool {
        matches!(self.0.options.controller.status().state, TurnState::Running | TurnState::Cancelling)
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
                this.0.options.controller.close(),
            )
            .await
            .unwrap_or_else(|_| Err(RuntimeError::plain("Shutdown deadline exceeded")));
            if result.is_err() {
                code = 1;
            }
            {
                let mut state = this.0.state.borrow_mut();
                if !state.output_failed {
                    let text = match result {
                        Ok(()) => "Kumi closed. Each Set's conversation continues next time.".into(),
                        Err(error) => {
                            code = 1;
                            head(&format!("[error] {}", safe_error_message(Some(&error.message()), &this.0.options.secrets)), 2048)
                        }
                    };
                    if let Some(p) = &mut state.presentation {
                        p.notice(this.0.options.output.as_ref(), &text, true);
                    }
                }
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
            InputEvent::Text { text } | InputEvent::Paste { text } => self.edit(|p| p.editor.insert(&text)),
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
        let controller = &self.0.options.controller;
        if command == "/quit" {
            self.finish(0).await;
            return Ok(());
        }
        if command == "/help" {
            self.notice(&help());
            return Ok(());
        }
        if command == "/update" {
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
                return Ok(());
            }
        }
        if command == "/stop" {
            if !controller.has_stop_live() {
                self.notice("[stop] Kumi can't stop Live here.");
                return Ok(());
            }
            if self.busy() {
                let _ = controller.cancel().await;
            }
            if !controller.stop_live().await? {
                self.notice("[stop] Kumi couldn't stop Live just now; press space in Live.");
            }
            return Ok(());
        }
        if command == "/status" {
            let status = controller.status();
            let library = library_line(controller.library().as_ref());
            self.notice(&format!(
                "[status] {}; MCP/Live: {}; turns {}{}; {}{}",
                enum_name(status.state),
                enum_name(status.connection),
                status.turns,
                status.max_turns.filter(|n| *n != 0).map(|n| format!("/{n}")).unwrap_or_default(),
                status.observation.as_deref().unwrap_or("No current Live observation"),
                library.map(|s| format!("; {s}")).unwrap_or_default()
            ));
            return Ok(());
        }
        if self.busy() {
            let mut state = self.0.state.borrow_mut();
            if !state.answering && !command.starts_with('/') && state.queued.is_none() {
                state.queued = Some(input.into());
                drop(state);
                self.notice("[waiting] Kumi is getting ready; your message goes as soon as it is.");
            } else {
                drop(state);
                self.notice("[busy] Busy; cancel first. No second turn was submitted.");
            }
            return Ok(());
        }
        let mut parts = command.split(|c: char| c.is_whitespace() || c == '\u{feff}').filter(|s| !s.is_empty());
        let verb = parts.next().unwrap_or("");
        let argument = parts.next();
        let result = self.command(verb, argument, command, input).await;
        if !self.0.state.borrow().closing {
            if let Err(error) = &result {
                self.error(error);
            }
            self.prompt();
        }
        Ok(())
    }
    async fn command(&self, verb: &str, argument: Option<&str>, command: &str, input: &str) -> Result<(), RuntimeError> {
        let controller = &self.0.options.controller;
        let models = &self.0.options.models;
        match verb{
 "/model"=>{self.model_command(argument).await?;},
 "/effort"=>{let choices=EFFORTS.map(|e|e.as_str()).join(", ");if let Some(argument)=argument{let effort=Effort::parse(argument);if argument!="default"&&effort.is_none(){self.notice(&format!("[effort] Choose one of {choices} or default."));}else{models.set_effort(effort).await?;self.notice(&format!("[effort] {argument}."));}}else{self.notice(&format!("[effort] {}. Choose one of {choices} or default.",models.current().effort.map(|e|e.as_str()).unwrap_or("the model's default")));}},
 "/fast"=>{let current=models.current();let on=!models.fast_enabled();match models.set_fast(on).await?{Some(tier)=>self.notice(&format!("[fast] {} is on{}.",tier.name,tier.description.map(|d|format!(": {d}")).unwrap_or_default())),None if on=>self.notice(&format!("[fast] {} has no faster tier.",current.name.or(current.model).unwrap_or_else(||"This model".into()))),None=>self.notice("[fast] Back to the standard tier."),}},
 "/memory"=>{let Some(memory)=controller.memory().await?else{self.notice("[memory] Kumi keeps no notes here.");return Ok(());};let list=|notes:&[kumi_runtime::core::contracts::MemoryNote]|{if notes.is_empty(){"none".into()}else{notes.iter().map(|n|format!("{}{} {}",n.id,if n.pinned{" (pinned)"}else{""},n.text)).collect::<Vec<_>>().join(" · ")}};self.notice(&format!("[memory] About you: {}",list(&memory.memory.producer)));self.notice(&if memory.saved{format!("[memory] About {}: {}",memory.set_name.as_deref().unwrap_or("this Set"),list(&memory.memory.set))}else{"[memory] This Set isn't saved yet; notes about it are kept once it is.".into()});if controller.has_techniques(){let techniques=controller.techniques().await?;self.notice(&format!("[memory] Techniques: {}",if techniques.is_empty(){"none yet".into()}else{techniques.iter().map(|t|format!("{} {} (for {})",t.id,t.name,t.fits)).collect::<Vec<_>>().join(" · ")}));}if controller.has_taste(){let taste=controller.taste().await?;self.notice(&format!("[memory] From your Sets: {}",if taste.is_empty(){"nothing yet".into()}else{taste.iter().enumerate().map(|(i,l)|format!("u{} {}",i+1,l.line)).collect::<Vec<_>>().join(" · ")}));self.0.state.borrow_mut().taste=taste;}},
 "/recipes"=>{let recipes=controller.recipes().await?;self.notice(&if recipes.is_empty(){"[recipes] None yet. Ask Kumi to save a way of working, or say \"watch me\" and do it in Live.".into()}else{format!("[recipes] {}",recipes.iter().map(|r|format!("{}{}: {}",r.name,if r.params.is_empty(){String::new()}else{format!(" (needs {})",r.params.iter().map(|p|p.name.as_str()).collect::<Vec<_>>().join(", "))},r.about)).collect::<Vec<_>>().join(" · "))});},
 "/conversations"=>{let kept=controller.conversations().await?;if let Some(argument)=argument{let n=parse_int(argument)-1.;let row=if n>=0.&&n.fract()==0.{kept.get(n as usize)}else{None};if let Some(row)=row{if row.current{self.notice("[conversations] That's this one.");}else if !controller.resume_conversation(&row.id).await?{self.notice("[conversations] That conversation isn't kept any more.");}}else{self.notice("[conversations] Use: /conversations <number>, with a number from /conversations.");}}else{self.notice(&if kept.is_empty(){"[conversations] None kept yet: a conversation is kept once you've asked something.".into()}else{format!("[conversations] {}. /conversations <number> goes back to one.",kept.iter().enumerate().map(|(i,r)|{let first=head(&sanitize_text(&r.first,&self.0.options.secrets).replace('\n'," "),80);format!("{}. {} ({}, {} {})",i+1,if first.is_empty(){"(nothing asked yet)"}else{&first},if r.current{"this one".into()}else{since(r.saved_at as f64,now_ms_f64())},r.turns,if r.turns==1{"request"}else{"requests"})}).collect::<Vec<_>>().join(" · "))});}},
 "/note"=>{let usage="[memory] Use: /note <id> <new words>, with an id from /memory.";let words=argument.and_then(|id|command.split_once(id)).map(|(_,words)|words.trim()).unwrap_or("");match argument.filter(|_|!words.is_empty()&&controller.has_change_note()){Some(id)=>match controller.change_note(id,NoteChange::Text(words.into())).await{Ok(Some(_))=>self.notice(&format!("[memory] Changed note {id}.")),Ok(None)=>self.notice(usage),Err(error)=>self.notice(&format!("[memory] {}",error.message()))},None=>self.notice(usage)}},
 "/pin"|"/unpin"=>{let pin=verb=="/pin";let usage=format!("[memory] Use: {verb} <id>, with a note's id from /memory.");match argument.filter(|_|controller.has_change_note()){Some(id)=>match controller.change_note(id,NoteChange::Pinned(pin)).await?{Some(_)=>self.notice(&if pin{format!("[memory] Pinned {id}: Kumi keeps it even when its memory is full.")}else{format!("[memory] Unpinned {id}.")}),None=>self.notice(&usage)},None=>self.notice(&usage)}},
 "/forget"=>{let invalid="[memory] Use: /forget <id>, with an id from /memory.";if argument.is_some_and(|a|a.starts_with('u'))&&controller.has_forget_taste(){if self.0.state.borrow().taste.is_empty(){let taste=controller.taste().await?;self.0.state.borrow_mut().taste=taste;}let n=number::parse(&argument.unwrap()[1..]).unwrap_or(f64::NAN)-1.;let line=if n>=0.&&n.fract()==0.{self.0.state.borrow().taste.get(n as usize).cloned()}else{None};if let Some(line)=line{if controller.forget_taste(&line.id).await?{self.notice(&format!("[memory] Forgot, from your Sets: {}",line.line));}else{self.notice(invalid);}}else{self.notice(invalid);}}else if argument.is_some_and(|a|a.starts_with('t'))&&controller.has_forget_technique(){if !controller.forget_technique(argument.unwrap()).await?{self.notice(invalid);}}else if let Some(argument)=argument{if controller.forget(argument).await?.is_none(){self.notice(invalid);}}else{self.notice(invalid);}},
 "/login"=>self.notice(&format!("[login] Sign in from a shell: {} login <provider> (openai-codex, anthropic, openai, opencode). The full-screen app signs in here.",*KUMI)),
 "/logout"=>{if let Some(provider)=argument.and_then(ProviderId::parse){self.notice(&if models.sign_out(provider).await?{format!("[logout] Signed out of {}.",provider.as_str())}else{format!("[logout] There was no sign-in for {} to remove.",provider.as_str())});}else{self.notice(&format!("[logout] Use: /logout <provider> ({}).",PROVIDERS.map(|p|p.as_str()).join(", ")));}},
 _=>{if command=="/undo"{if let Some(change)=controller.undo(None).await?{self.notice(&if enum_name(change.state)=="undone"{format!("[undo] Undid: {}",change.title)}else{string::trim(&format!("[undo] Kept: {}. {}",change.title,change.note.unwrap_or_default())).into()});}}else if command=="/refresh"{controller.refresh().await?;}else if command=="/reconnect"&&controller.has_reconnect(){controller.reconnect().await?;}else if command=="/new"{self.notice("── New conversation. Kumi won't use what's above ──");controller.new_conversation().await?;}else if command.starts_with('/'){self.notice("Unknown command. Use /help.");}else{self.0.state.borrow_mut().answering=true;let result=controller.submit(input,None).await;self.0.state.borrow_mut().answering=false;result?;}}
 }
        Ok(())
    }
    async fn model_command(&self, argument: Option<&str>) -> Result<(), RuntimeError> {
        let models = &self.0.options.models;
        let Some(argument) = argument else {
            let current = models.current();
            let mut available =
                models.providers().await?.into_iter().filter(|p| p.signed_in).map(|p| p.id.as_str().to_string()).collect::<Vec<_>>();
            available.extend(models.local().await.into_iter().filter(|s| s.running).map(|s| s.id));
            self.notice(&format!(
                "[model] {}{}. List a provider's with /model <provider> ({}); choose with /model <provider>/<model>.",
                current.model.as_deref().unwrap_or("none chosen"),
                current.effort.map(|e| format!(", effort {}", e.as_str())).unwrap_or_default(),
                if available.is_empty() { "sign in first, or open Ollama or LM Studio".into() } else { available.join(", ") }
            ));
            return Ok(());
        };
        if !argument.contains('/') && (ProviderId::parse(argument).is_some() || models.local().await.iter().any(|s| s.id == argument)) {
            let listed = models.models(argument, false).await?;
            self.notice(&format!(
                "[model] {argument}: {}",
                if listed.is_empty() {
                    "no models listed".into()
                } else {
                    listed.iter().map(|m| m.model.as_str()).collect::<Vec<_>>().join(", ")
                }
            ));
        } else {
            let note = models.choose(argument).await?;
            let fast = models.current().fast.map(|tier| format!(" {} is on; /fast turns it off.", tier.name)).unwrap_or_default();
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
            self.0.options.models.current().model.as_deref().unwrap_or("no model chosen yet"),
            if self.0.options.mode == "inference-only" { "MCP disconnected / No Live access" } else { "MCP connecting / Live unverified" }
        ));
        self.notice("Each Set's conversations are kept: its latest continues next time, and /conversations goes back to earlier ones. /help for commands.");
        if let Some(notice) = self.0.options.startup_notice.as_ref().filter(|s| !s.is_empty()) {
            self.notice(notice);
        }
        let weak = Rc::downgrade(&self.0);
        active.resume(Rc::new(move |bytes| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).bytes(bytes);
            }
        }));
        if self.0.options.models.current().model.is_none() {
            let this = self.clone();
            tokio::task::spawn_local(async move {
                if let Ok(chosen) = this.0.options.models.choose_default().await {
                    if !this.0.state.borrow().closing {
                        this.notice(&if let Some(chosen)=chosen{if let Some(where_)=chosen.model.r#where.filter(|s|!s.is_empty()){format!("[model] {}, in {} {where_}. /model changes it.{}",chosen.model.id,this.0.options.models.provider_name(&chosen.model.provider),chosen.note.filter(|s|!s.is_empty()).map(|s|format!(" {s}")).unwrap_or_default())}else{format!("[model] {}, the first {} lists. /model changes it.",chosen.model.id,chosen.model.provider)}}else{format!("[model] Not signed in to a provider yet. Sign in with: {} login <provider>, then /model; or open Ollama or LM Studio.",*KUMI)});
                    }
                }
            });
        }
        let this = self.clone();
        tokio::task::spawn_local(async move {
            if !this.0.state.borrow().closing {
                if let Err(error) = this.0.options.controller.start().await {
                    if !this.0.state.borrow().closing {
                        this.error(&error);
                        this.finish(1).await;
                    }
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
                drop(state);
                if let Some(line) = queued {
                    let this = self.clone();
                    tokio::task::spawn_local(async move {
                        this.spawn_submit(line);
                    });
                }
            }
            "connection" => self.notice(&format!(
                "[connection] MCP/Live: {}{}",
                get("state"),
                if get("state") != "connected" { "; no verified current Live observation" } else { "" }
            )),
            "observation" => self.notice(&format!("[observation] {}", get("label"))),
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
                let title = str_(&c["title"]);
                let note = str_(&c["note"]);
                let text = match str_(&c["state"]) {
                    "applied" => Some(format!("[change] {title} (/undo takes it back)")),
                    "unsure" => Some(format!("[change] Check Live: {title}. {}", c["note"].as_str().unwrap_or("Live didn't confirm it."))),
                    "kept" => Some(string::trim(&format!("[change] Kept: {title}. {note}")).into()),
                    "expired" => Some(string::trim(&format!("[change] No undo anymore: {title}. {note}")).into()),
                    "heard" => Some(format!("[heard] {title}{}", c.get("score").map(|n| format!(" · {}%", num(n))).unwrap_or_default())),
                    _ => None,
                };
                if let Some(text) = text {
                    self.notice(&text);
                }
            }
            "notice" => self.notice(get("message")),
            "error" => {
                self.error(&RuntimeError::plain(get("message")));
                self.0.state.borrow_mut().text.discard();
                if get("kind") == "auth" && ProviderId::parse(get("provider")).is_some() {
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
                    self.notice(&format!(
                        "[wait] {}; trying again in {} s",
                        get("reason"),
                        (value["waitMs"].as_f64().unwrap_or(0.) / 1000.).ceil().max(1.)
                    ));
                }
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
                if get("scope") == "producer" { "about you" } else { "about this Set" },
                str_(&value["note"]["text"]),
                if value["pending"] == true { " (kept once the Set is saved)" } else { "" }
            )),
            "forgot" => self.notice(&format!("[memory] Forgot: {}", str_(&value["note"]["text"]))),
            "action" => self.notice(&format!("[live] {}", get("title"))),
            "watching" => self.notice(if value["on"] == true {
                "[live] Kumi is watching the Set; do it in Live, then tell Kumi you're done."
            } else {
                "[live] Kumi stopped watching."
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
            if let Err(error) = this.0.options.controller.cancel().await {
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
