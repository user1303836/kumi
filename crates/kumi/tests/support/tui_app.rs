#![allow(dead_code)]
use async_trait::async_trait;
use kumi::{
    input::{ByteListener, TerminalInput},
    terminal::Terminal,
    tui::{
        alerts::TerminalProfile,
        app::{TuiApp, TuiOptions},
        icons::IconStyle,
        style::ColorDepth,
        tty::TtyOutput,
    },
};
use kumi_runtime::core::{contracts::*, errors::RuntimeError};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
#[path = "vt.rs"]
pub mod vt;
#[derive(Default)]
pub struct Input {
    pub raw: Cell<bool>,
    data: RefCell<Option<ByteListener>>,
    end: RefCell<Option<Rc<dyn Fn()>>>,
}
impl TerminalInput for Input {
    fn is_tty(&self) -> bool {
        true
    }
    fn is_raw(&self) -> bool {
        self.raw.get()
    }
    fn set_raw_mode(&self, on: bool) -> std::io::Result<()> {
        self.raw.set(on);
        Ok(())
    }
    fn resume(&self, f: ByteListener) {
        *self.data.borrow_mut() = Some(f)
    }
    fn pause(&self) {
        self.data.borrow_mut().take();
    }
    fn on_end(&self, f: Rc<dyn Fn()>) {
        *self.end.borrow_mut() = Some(f)
    }
}
impl Input {
    pub fn write(&self, s: &str) {
        let f = self.data.borrow().clone();
        if let Some(f) = f {
            f(s.as_bytes())
        }
    }
}
pub struct Out {
    pub text: RefCell<String>,
    written: tokio::sync::Notify,
    pub width: Cell<i32>,
    pub height: Cell<i32>,
    resize: RefCell<Option<Rc<dyn Fn()>>>,
}
impl TtyOutput for Out {
    fn is_tty(&self) -> bool {
        true
    }
    fn columns(&self) -> Option<i32> {
        Some(self.width.get())
    }
    fn rows(&self) -> Option<i32> {
        Some(self.height.get())
    }
    fn write(&self, s: &str) {
        self.text.borrow_mut().push_str(s);
        self.written.notify_one();
    }
    fn watch_resize(&self, f: Rc<dyn Fn()>) {
        *self.resize.borrow_mut() = Some(f)
    }
    fn unwatch_resize(&self) {
        self.resize.borrow_mut().take();
    }
}
pub struct Control {
    pub extra: RefCell<Value>,
    pub aside_gate: RefCell<Option<kumi_common::abort::Signal>>,
    pub calls: RefCell<Vec<String>>,
    called: tokio::sync::Notify,
    pub state: Cell<TurnState>,
    pub undo: RefCell<Option<ChangeRecord>>,
    pub emit: RefCell<Option<Rc<dyn Fn(SessionEvent)>>>,
    pub stop: Cell<bool>,
    pub pins: RefCell<Vec<Option<PinnedNode>>>,
    pub steer_enabled: Cell<bool>,
    pub steer_result: Cell<bool>,
    pub memory: RefCell<Option<MemoryView>>,
    pub conversations: RefCell<Vec<ConversationSummary>>,
    pub library: RefCell<Option<LibraryStatus>>,
    pub tree: RefCell<Option<DeviceTree>>,
    pub clip: RefCell<Option<ClipView>>,
    pub session: RefCell<Option<SessionStrip>>,
    pub arrangement: RefCell<Option<ArrangementStrip>>,
    /// What /export and /share get as the conversation going on now.
    pub export: RefCell<Option<ConversationToExport>>,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            extra: RefCell::new(serde_json::json!({})),
            aside_gate: RefCell::default(),
            calls: RefCell::default(),
            called: tokio::sync::Notify::new(),
            state: Cell::new(TurnState::Idle),
            undo: RefCell::default(),
            emit: RefCell::default(),
            stop: Cell::new(false),
            pins: RefCell::default(),
            steer_enabled: Cell::new(false),
            steer_result: Cell::new(true),
            memory: RefCell::default(),
            conversations: RefCell::default(),
            library: RefCell::default(),
            tree: RefCell::default(),
            clip: RefCell::default(),
            session: RefCell::default(),
            arrangement: RefCell::default(),
            export: RefCell::default(),
        }
    }
}
impl Control {
    pub fn set(&self, key: &str, value: Value) {
        self.extra.borrow_mut()[key] = value;
    }
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.extra.borrow().get(key).cloned().map(|v| serde_json::from_value(v).unwrap())
    }
    pub fn enabled(&self, key: &str) -> bool {
        self.extra.borrow().get(key).is_some()
    }
    pub fn emit(&self, e: SessionEvent) {
        let f = self.emit.borrow().clone();
        if let Some(f) = f {
            f(e)
        }
    }
    pub fn call(&self, s: impl Into<String>) {
        self.calls.borrow_mut().push(s.into());
        self.called.notify_one();
    }
}
#[async_trait(?Send)]
impl SessionController for Control {
    async fn start(&self) -> Result<(), RuntimeError> {
        self.call("start");
        Ok(())
    }
    async fn reconfigure(&self) -> Result<(), RuntimeError> {
        self.call("reconfigure");
        Ok(())
    }
    async fn submit(&self, s: &str, pin: Option<PinnedNode>) -> Result<(), RuntimeError> {
        if let Some(error) = self.get::<String>("submit-error") {
            return Err(RuntimeError::plain(error));
        }
        self.call(format!("submit:{s}"));
        self.pins.borrow_mut().push(pin);
        Ok(())
    }
    fn has_attachments(&self) -> bool {
        true
    }
    async fn submit_with(&self, s: &str, pin: Option<PinnedNode>, attachments: Vec<Attachment>) -> Result<(), RuntimeError> {
        if attachments.is_empty() {
            return self.submit(s, pin).await;
        }
        if let Some(error) = self.get::<String>("submit-error") {
            return Err(RuntimeError::plain(error));
        }
        let files: Vec<_> = attachments.iter().map(|a| format!("{} {} {}", a.name, a.media_type, a.bytes)).collect();
        self.call(format!("submit-with:{s} [{}]", files.join(", ")));
        self.pins.borrow_mut().push(pin);
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        self.call("refresh");
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        self.call("new");
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        self.call("cancel");
        self.state.set(TurnState::Idle);
        if self.enabled("cancel-idle") {
            self.emit(SessionEvent::State { state: TurnState::Idle });
        }
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.call("close");
        self.state.set(TurnState::Closed);
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        SessionStatus { state: self.state.get(), connection: ConnectionState::Connected, turns: 0, max_turns: Some(30), observation: None }
    }
    async fn undo(&self, id: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        self.call(format!("undo:{}", id.unwrap_or("last")));
        let c = self.undo.borrow().clone();
        if let Some(change) = c.clone() {
            self.emit(SessionEvent::Change { change });
        }
        Ok(c)
    }

    fn has_reconnect(&self) -> bool {
        self.enabled("reconnect")
    }
    async fn reconnect(&self) -> Result<(), RuntimeError> {
        self.call("reconnect");
        Ok(())
    }
    fn has_aside(&self) -> bool {
        self.enabled("aside")
    }
    async fn aside(&self, question: &str, on_text: OnText, signal: Option<kumi_common::abort::Signal>) -> Result<String, RuntimeError> {
        self.call(format!("aside:{question}"));
        let text = self.get::<String>("aside").unwrap();
        on_text(&text);
        let gate = self.aside_gate.borrow().clone();
        if let Some(gate) = gate {
            if let Some(signal) = signal {
                tokio::select! {_=gate.cancelled()=>{},_=signal.cancelled()=>{}}
            } else {
                gate.cancelled().await;
            }
        }
        Ok(text)
    }
    fn has_goal(&self) -> bool {
        self.enabled("goal")
    }
    async fn goal(&self, text: Option<&str>) -> Result<(), RuntimeError> {
        self.call(format!("goal:{}", text.unwrap_or("")));
        Ok(())
    }
    fn has_stop_goal(&self) -> bool {
        self.enabled("goal")
    }
    async fn stop_goal(&self) -> Result<bool, RuntimeError> {
        self.call("stop-goal");
        Ok(true)
    }
    fn has_stop_loop(&self) -> bool {
        self.enabled("goal")
    }
    async fn stop_loop(&self) -> Result<bool, RuntimeError> {
        self.call("stop-loop");
        Ok(true)
    }
    fn has_recipes(&self) -> bool {
        self.enabled("recipes")
    }
    async fn recipes(&self) -> Result<Vec<RecipeSummary>, RuntimeError> {
        Ok(self.get("recipes").unwrap_or_default())
    }
    fn has_instant(&self) -> bool {
        self.enabled("instant")
    }
    async fn instant(&self, text: &str) -> Result<Option<InstantOutcome>, RuntimeError> {
        self.call(format!("instant:{text}"));
        // A phrase the fake can run lands as Kumi's would: its change, then what it said. Without one, the model takes it.
        let Some(change) = self.get::<ChangeRecord>("instant-change") else { return Ok(None) };
        self.emit(SessionEvent::Change { change: change.clone() });
        Ok(Some(InstantOutcome { text: change.title, is_error: false }))
    }
    fn has_run_recipe(&self) -> bool {
        self.enabled("recipes")
    }
    async fn run_recipe(&self, name: &str, with: JsonObject) -> Result<RecipeOutcome, RuntimeError> {
        self.call(if with.is_empty() { format!("run-recipe:{name}") } else { format!("run-recipe:{name} {}", Value::Object(with)) });
        Ok(self.get("recipe-result").unwrap())
    }
    fn has_forget_recipe(&self) -> bool {
        self.enabled("recipes")
    }
    async fn forget_recipe(&self, name: &str) -> Result<bool, RuntimeError> {
        self.call(format!("forget-recipe:{name}"));
        // A store that can't be written, when a test says so.
        match self.get::<String>("forget-fails") {
            Some(reason) => Err(RuntimeError::plain(reason)),
            None => Ok(true),
        }
    }
    fn has_techniques(&self) -> bool {
        self.enabled("techniques")
    }
    async fn techniques(&self) -> Result<Vec<TechniqueSummary>, RuntimeError> {
        Ok(self.get("techniques").unwrap_or_default())
    }
    fn has_forget_technique(&self) -> bool {
        self.enabled("techniques")
    }
    async fn forget_technique(&self, id: &str) -> Result<bool, RuntimeError> {
        self.call(format!("forget-technique:{id}"));
        Ok(true)
    }
    fn has_answer_technique(&self) -> bool {
        self.enabled("techniques")
    }
    async fn answer_technique(&self, keep: bool) -> Result<bool, RuntimeError> {
        self.call(format!("answer-technique:{}", if keep { "yes" } else { "no" }));
        Ok(self.get("technique-waiting").unwrap_or(true))
    }
    fn picked(&self, pick: Picked) {
        self.call(match pick {
            Picked::Answer { question, options, index } => format!("picked:answer:{index} of {}:{question}", options.len()),
            Picked::Technique { name, keep } => format!("picked:technique:{name}:{}", if keep { "yes" } else { "no" }),
        });
    }
    fn has_taste(&self) -> bool {
        self.enabled("taste")
    }
    async fn taste(&self) -> Result<Vec<KeptLine>, RuntimeError> {
        Ok(self.get("taste").unwrap_or_default())
    }
    fn has_forget_taste(&self) -> bool {
        self.enabled("taste")
    }
    async fn forget_taste(&self, id: &str) -> Result<bool, RuntimeError> {
        self.call(format!("forget-taste:{id}"));
        Ok(true)
    }
    fn has_forget(&self) -> bool {
        self.memory.borrow().is_some()
    }
    async fn forget(&self, id: &str) -> Result<Option<MemoryNote>, RuntimeError> {
        self.call(format!("forget:{id}"));
        Ok(self.get("forgot"))
    }
    fn has_change_note(&self) -> bool {
        self.memory.borrow().is_some()
    }
    async fn change_note(&self, id: &str, change: NoteChange) -> Result<Option<MemoryNote>, RuntimeError> {
        self.call(format!("change-note:{id} {change:?}"));
        let mut view = self.memory.borrow_mut();
        let Some(view) = view.as_mut() else { return Ok(None) };
        let Some(note) = view.memory.producer.iter_mut().chain(view.memory.set.iter_mut()).find(|n| n.id == id) else {
            return Ok(None);
        };
        match change {
            NoteChange::Text(text) => note.text = text,
            NoteChange::Pinned(pinned) => note.pinned = pinned,
        }
        Ok(Some(note.clone()))
    }
    fn answering_fallback(&self) -> Option<String> {
        self.get::<String>("answering")
    }
    fn has_daw_message(&self) -> bool {
        true
    }
    async fn daw_message(&self, text: &str) -> Result<bool, RuntimeError> {
        self.call(format!("daw-message:{text}"));
        Ok(true)
    }
    async fn daw_recording(&self) -> Result<Option<bool>, RuntimeError> {
        Ok(self.get::<bool>("recording"))
    }
    fn has_jobs(&self) -> bool {
        self.enabled("jobs")
    }
    fn jobs(&self) -> Vec<kumi_runtime::core::jobs::JobStatus> {
        self.get("jobs").unwrap_or_default()
    }
    fn job(&self, id: u32) -> Option<kumi_runtime::core::jobs::JobDetail> {
        self.get::<Vec<kumi_runtime::core::jobs::JobDetail>>("job-details").unwrap_or_default().into_iter().find(|d| d.status.id == id)
    }
    fn stop_job(&self, id: u32) -> bool {
        self.call(format!("stop-job:{id}"));
        true
    }
    fn run_job_again(&self, id: u32) -> Result<u32, String> {
        self.call(format!("run-again:{id}"));
        Ok(id + 100)
    }
    fn background(&self) -> Result<u32, String> {
        self.call("background");
        self.get::<String>("background-refused").map_or(Ok(7), Err)
    }
    fn can_background(&self) -> bool {
        self.enabled("can-background")
    }
    fn carry_on(&self, job: u32) -> bool {
        self.call(format!("carry-on:{job}"));
        true
    }
    fn has_stop_live(&self) -> bool {
        self.stop.get()
    }
    async fn stop_live(&self) -> Result<bool, RuntimeError> {
        self.call("stop-live");
        let success = self.get::<bool>("stop-result").unwrap_or(true);
        if success {
            self.emit(
                serde_json::from_value(serde_json::json!({"type":"action","title":"Stopped","playing":false,"recording":false})).unwrap(),
            );
        }
        Ok(success)
    }
    fn has_steer(&self) -> bool {
        self.steer_enabled.get()
    }
    fn steer(&self, text: &str) -> bool {
        self.call(format!("steer:{text}"));
        self.steer_result.get()
    }
    fn has_memory(&self) -> bool {
        self.memory.borrow().is_some()
    }
    async fn memory(&self) -> Result<Option<MemoryView>, RuntimeError> {
        Ok(self.memory.borrow().clone())
    }
    fn has_conversations(&self) -> bool {
        !self.conversations.borrow().is_empty()
    }
    async fn conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        Ok(self.conversations.borrow().clone())
    }
    fn has_resume_conversation(&self) -> bool {
        self.has_conversations()
    }
    async fn resume_conversation(&self, id: &str) -> Result<bool, RuntimeError> {
        self.call(format!("resume:{id}"));
        Ok(true)
    }
    async fn all_conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        let rows: Vec<ConversationSummary> = self.get("all").unwrap_or_else(|| self.conversations.borrow().clone());
        // As the session marks them: `current` is the conversation going on, not each Set's own latest.
        Ok(match self.get::<String>("going") {
            Some(going) => rows.into_iter().map(|row| ConversationSummary { current: row.id == going, ..row }).collect(),
            None => rows,
        })
    }
    async fn find_conversation(&self, word: &str) -> Result<ConversationMatch, RuntimeError> {
        let all = self.all_conversations().await?;
        let found: Vec<_> = all.into_iter().filter(|row| row.id.starts_with(word) || row.name.as_deref() == Some(word)).collect();
        Ok(match found.len() {
            0 => ConversationMatch::None,
            1 => ConversationMatch::One(found[0].clone()),
            _ => ConversationMatch::Several(found),
        })
    }
    async fn rename_conversation(&self, id: Option<&str>, name: Option<&str>) -> Result<bool, RuntimeError> {
        self.call(format!("rename:{}:{}", id.unwrap_or("this"), name.unwrap_or("")));
        Ok(true)
    }
    async fn pin_conversation(&self, id: &str, pinned: bool) -> Result<bool, RuntimeError> {
        self.call(format!("pin:{id}:{pinned}"));
        Ok(true)
    }
    async fn delete_conversation(&self, id: &str) -> Result<bool, RuntimeError> {
        self.call(format!("delete:{id}"));
        Ok(true)
    }
    async fn fork_conversation(&self, name: Option<&str>) -> Result<Option<String>, RuntimeError> {
        self.call(format!("fork:{}", name.unwrap_or("")));
        Ok(Some("fork01".into()))
    }
    async fn answer_set(&self, choice: SetChoice) -> Result<(), RuntimeError> {
        self.call(format!("answer:{}", serde_json::to_value(choice).unwrap().as_str().unwrap()));
        Ok(())
    }
    async fn conversation_now(&self) -> Option<ConversationNow> {
        self.get("now")
    }
    async fn conversation_lines(&self, _: &str) -> Result<Vec<TranscriptLine>, RuntimeError> {
        Ok(self.get("lines").unwrap_or_default())
    }
    async fn conversation_to_export(&self) -> Option<ConversationToExport> {
        self.call("export");
        self.export.borrow().clone()
    }
    fn has_library(&self) -> bool {
        self.library.borrow().is_some()
    }
    fn library(&self) -> Option<LibraryStatus> {
        self.library.borrow().clone()
    }
    fn has_device_tree(&self) -> bool {
        self.tree.borrow().is_some()
    }
    async fn device_tree(&self, s: &str) -> Result<Option<DeviceTree>, RuntimeError> {
        self.call(format!("tree:{s}"));
        Ok(self.tree.borrow().clone())
    }
    fn has_clip_view(&self) -> bool {
        self.clip.borrow().is_some()
    }
    async fn clip_view(&self, s: &str) -> Result<Option<ClipView>, RuntimeError> {
        self.call(format!("clip:{s}"));
        Ok(self.clip.borrow().clone())
    }
    fn has_session_strip(&self) -> bool {
        self.session.borrow().is_some()
    }
    async fn session_strip(&self, _track: &str, scene: f64) -> Result<Option<SessionStrip>, RuntimeError> {
        self.call(format!("session:{scene}"));
        Ok(self.session.borrow().clone())
    }
    fn has_arrangement_strip(&self) -> bool {
        self.arrangement.borrow().is_some()
    }
    async fn arrangement_strip(&self) -> Result<Option<ArrangementStrip>, RuntimeError> {
        self.call("arrangement");
        Ok(self.arrangement.borrow().clone())
    }
}
pub struct Harness {
    pub input: Rc<Input>,
    pub output: Rc<Out>,
    pub control: Rc<Control>,
    pub app: TuiApp,
    pub browsed: Rc<RefCell<Vec<String>>>,
    /// The notification programs Kumi ran (osascript, notify-send), with their arguments.
    pub notified: Rc<RefCell<Vec<(String, Vec<String>)>>>,
    vt: RefCell<vt::VirtualTerminal>,
    consumed: Cell<usize>,
}
impl Harness {
    pub fn new(w: i32, h: i32) -> Self {
        Self::with(w, h, Rc::new(Control::default()), |_| {})
    }
    pub fn with(w: i32, h: i32, control: Rc<Control>, configure: impl FnOnce(&mut TuiOptions)) -> Self {
        let input = Rc::new(Input::default());
        let output = Rc::new(Out {
            text: RefCell::default(),
            written: tokio::sync::Notify::new(),
            width: Cell::new(w),
            height: Cell::new(h),
            resize: RefCell::default(),
        });
        let browsed = Rc::new(RefCell::new(vec![]));
        let notified = Rc::new(RefCell::new(vec![]));
        let mut opts = TuiOptions::new(control.clone(), input.clone(), output.clone(), "live");
        // No status or notification unless a test asks: none of this machine's terminal.
        opts.terminal = Some(TerminalProfile::quiet());
        let n = notified.clone();
        opts.run_notifier = Some(Rc::new(move |program, args| n.borrow_mut().push((program.into(), args))));
        opts.secrets = vec!["private-token".into()];
        opts.color_depth = Some(ColorDepth::Truecolor);
        opts.icons = Some(IconStyle::Glyphs);
        opts.frame_ms = Some(1.0);
        // Pictures as cells, whatever terminal runs the tests.
        opts.graphics = Some(kumi::tui::pictures::Graphics::Cells);
        opts.close_timeout_ms = Some(100);
        let b = browsed.clone();
        opts.open_browser = Some(Rc::new(move |s| b.borrow_mut().push(s.into())));
        configure(&mut opts);
        let app = TuiApp::new(opts);
        let a = app.clone();
        *control.emit.borrow_mut() = Some(Rc::new(move |e| a.handle_event(e)));
        Self { input, output, control, app, browsed, notified, vt: RefCell::new(vt::VirtualTerminal::new(w, h)), consumed: Cell::new(0) }
    }
    pub async fn start(&self) {
        let _done = self.app.run();
        delay(5).await;
    }
    pub fn screen(&self) -> Vec<String> {
        self.app.flush();
        let written = self.output.text.borrow();
        self.vt.borrow_mut().write(&written[self.consumed.get()..]);
        self.consumed.set(written.len());
        self.vt.borrow().lines()
    }
    pub fn emit(&self, value: Value) {
        let event: SessionEvent = serde_json::from_value(value).unwrap();
        if let SessionEvent::State { state } = event {
            self.control.state.set(state);
        }
        self.app.handle_event(event);
    }
    pub fn connect(&self) {
        self.emit(serde_json::json!({"type":"connection","state":"connected"}));
        self.emit(serde_json::json!({"type":"observation","label":"Current open Set: Night Drive — Remote Script · real-live"}));
    }
    pub async fn type_text(&self, s: &str) {
        self.input.write(s);
        delay(45).await;
    }
    pub fn resize(&self, w: i32, h: i32) {
        self.output.width.set(w);
        self.output.height.set(h);
        *self.vt.borrow_mut() = vt::VirtualTerminal::new(w, h);
        self.consumed.set(self.output.text.borrow().len());
        let f = self.output.resize.borrow().clone();
        if let Some(f) = f {
            f()
        }
    }
    pub fn calls(&self) -> Vec<String> {
        self.control.calls.borrow().clone()
    }
    pub async fn wait_for_call(&self, expected: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let called = self.control.called.notified();
                if self.control.calls.borrow().iter().any(|call| call == expected) {
                    return;
                }
                called.await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("Missing controller call {expected:?}: {:?}", self.calls()));
    }
    /// Waits until `count` controller calls start with `prefix`.
    pub async fn wait_for_calls(&self, prefix: &str, count: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let called = self.control.called.notified();
                if self.control.calls.borrow().iter().filter(|call| call.starts_with(prefix)).count() >= count {
                    return;
                }
                called.await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("Missing {count} controller calls starting {prefix:?}: {:?}", self.calls()));
    }
    async fn wait_for_screen(&self, description: &str, ready: impl Fn(&[String]) -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let written = self.output.written.notified();
                if ready(&self.screen()) {
                    return;
                }
                written.await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("Waiting for {description}:\n{}", self.screen().join("\n")));
    }
    pub async fn wait_for(&self, text: &str) {
        self.wait_for_screen(&format!("{text:?}"), |lines| has(lines, text)).await;
    }
    pub async fn wait_until_hidden(&self, text: &str) {
        self.wait_for_screen(&format!("{text:?} to disappear"), |lines| !has(lines, text)).await;
    }
    pub fn has_selection_highlight(&self) -> bool {
        self.screen();
        let [r, g, b] = kumi::tui::style::palette::SELECTED;
        let background = format!("{r},{g},{b}");
        self.vt.borrow().styles.iter().flatten().any(|style| style.split('|').nth(1) == Some(background.as_str()))
    }
    /// The line the selection band is on (the keyboard's row in the tabs or a panel), if any.
    pub fn selected_line(&self) -> Option<String> {
        let lines = self.screen();
        let [r, g, b] = kumi::tui::style::palette::SELECTED;
        let background = format!("{r},{g},{b}");
        let vt = self.vt.borrow();
        vt.styles
            .iter()
            .position(|row| row.iter().any(|style| style.split('|').nth(1) == Some(background.as_str())))
            .map(|at| lines[at].clone())
    }
    pub async fn wait_until_selection_clears(&self) {
        self.wait_for_screen("selection highlight to disappear", |_| !self.has_selection_highlight()).await;
    }
    pub fn has(&self, text: &str) {
        let lines = self.screen();
        assert!(has(&lines, text), "Missing {text:?}:\n{}", lines.join("\n"));
    }
    pub async fn close(&self) {
        self.app.close().await;
        self.control.emit.borrow_mut().take();
    }
}
pub fn has(lines: &[String], text: &str) -> bool {
    lines.iter().any(|line| line.contains(text))
}
pub fn click(lines: &[String], row: usize, text: &str) -> String {
    let col = lines[row].find(text).unwrap();
    let col = lines[row][..col].chars().count();
    format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", col + 1, row + 1, col + 1, row + 1)
}
pub async fn delay(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}
