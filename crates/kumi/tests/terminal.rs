#[path = "support/models.rs"]
mod models;
use async_trait::async_trait;
use futures::FutureExt;
use kumi::{
    input::{ByteListener, TerminalInput},
    terminal::{create_terminal, PlainTerminal, Terminal, TerminalOptions},
    tui::tty::TtyOutput,
    update::UpdateControl,
    willington::WillingtonControl,
};
use kumi_common::abort::Signal;
use kumi_runtime::core::{contracts::*, errors::RuntimeError};
use models::FakeModels;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[derive(Default)]
struct Input {
    tty: bool,
    raw: Cell<bool>,
    data: RefCell<Option<ByteListener>>,
    end: RefCell<Option<Rc<dyn Fn()>>>,
}
impl TerminalInput for Input {
    fn is_tty(&self) -> bool {
        self.tty
    }
    fn is_raw(&self) -> bool {
        self.raw.get()
    }
    fn set_raw_mode(&self, on: bool) -> std::io::Result<()> {
        self.raw.set(on);
        Ok(())
    }
    fn resume(&self, f: ByteListener) {
        *self.data.borrow_mut() = Some(f);
    }
    fn pause(&self) {
        self.data.borrow_mut().take();
    }
    fn on_end(&self, f: Rc<dyn Fn()>) {
        *self.end.borrow_mut() = Some(f);
    }
}
impl Input {
    fn write(&self, s: &str) {
        let callback = self.data.borrow().clone();
        if let Some(callback) = callback {
            callback(s.as_bytes());
        }
    }
    fn end(&self) {
        let callback = self.end.borrow().clone();
        if let Some(callback) = callback {
            callback();
        }
    }
}
#[derive(Default)]
struct Out {
    tty: bool,
    text: RefCell<String>,
}
impl TtyOutput for Out {
    fn is_tty(&self) -> bool {
        self.tty
    }
    fn columns(&self) -> Option<i32> {
        Some(40)
    }
    fn rows(&self) -> Option<i32> {
        Some(24)
    }
    fn write(&self, s: &str) {
        self.text.borrow_mut().push_str(s);
    }
}
struct Control {
    calls: RefCell<Vec<String>>,
    state: Cell<TurnState>,
    hold: Cell<bool>,
    stop: Cell<bool>,
    hang_close: Cell<bool>,
    release: RefCell<Vec<Signal>>,
    emit: RefCell<Option<Rc<dyn Fn(SessionEvent)>>>,
    memory: RefCell<Option<MemoryView>>,
    taste: RefCell<Vec<KeptLine>>,
    library: RefCell<Option<LibraryStatus>>,
    conversations: RefCell<Vec<ConversationSummary>>,
    resumed: RefCell<Vec<String>>,
    reconnect: Cell<bool>,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            calls: RefCell::new(vec![]),
            state: Cell::new(TurnState::Idle),
            hold: Cell::new(false),
            stop: Cell::new(false),
            hang_close: Cell::new(false),
            release: RefCell::new(vec![]),
            emit: RefCell::new(None),
            memory: RefCell::new(None),
            taste: RefCell::new(vec![]),
            library: RefCell::new(None),
            conversations: RefCell::new(vec![]),
            resumed: RefCell::new(vec![]),
            reconnect: Cell::new(false),
        }
    }
}
impl Control {
    fn emit(&self, event: SessionEvent) {
        let emit = self.emit.borrow().clone();
        if let Some(emit) = emit {
            emit(event);
        }
    }
    fn release(&self) {
        for gate in self.release.borrow_mut().drain(..) {
            gate.cancel();
        }
    }
}
#[async_trait(?Send)]
impl SessionController for Control {
    async fn start(&self) -> Result<(), RuntimeError> {
        self.calls.borrow_mut().push("start".into());
        Ok(())
    }
    async fn submit(&self, text: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        self.calls.borrow_mut().push(format!("submit:{text}"));
        self.state.set(TurnState::Running);
        self.emit(SessionEvent::State { state: TurnState::Running });
        if self.hold.get() {
            let gate = Signal::new();
            self.release.borrow_mut().push(gate.clone());
            gate.cancelled().await;
        }
        self.state.set(TurnState::Idle);
        self.emit(SessionEvent::State { state: TurnState::Idle });
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        self.calls.borrow_mut().push("refresh".into());
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        self.calls.borrow_mut().push("new".into());
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        self.calls.borrow_mut().push("cancel".into());
        self.state.set(TurnState::Idle);
        self.release();
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.calls.borrow_mut().push("close".into());
        if self.hang_close.get() {
            std::future::pending::<()>().await;
        }
        self.state.set(TurnState::Closed);
        self.release();
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        SessionStatus {
            state: self.state.get(),
            connection: ConnectionState::Disconnected,
            turns: 0,
            max_turns: Some(30),
            observation: None,
        }
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        self.calls.borrow_mut().push("undo".into());
        Ok(Some(serde_json::from_value(json!({"id":"c1","family":"tempo","title":"Tempo 120 → 124 BPM","state":"undone","at":1})).unwrap()))
    }
    fn has_memory(&self) -> bool {
        true
    }
    async fn memory(&self) -> Result<Option<MemoryView>, RuntimeError> {
        Ok(self.memory.borrow().clone())
    }
    fn has_change_note(&self) -> bool {
        true
    }
    async fn change_note(&self, id: &str, change: NoteChange) -> Result<Option<MemoryNote>, RuntimeError> {
        let mut memory = self.memory.borrow_mut();
        let memory = memory.as_mut().unwrap();
        let Some(note) = memory.memory.producer.iter_mut().chain(memory.memory.set.iter_mut()).find(|n| n.id == id) else {
            return Ok(None);
        };
        match change {
            NoteChange::Text(text) => note.text = text,
            NoteChange::Pinned(pinned) => note.pinned = pinned,
        }
        Ok(Some(note.clone()))
    }
    async fn forget(&self, id: &str) -> Result<Option<MemoryNote>, RuntimeError> {
        let note = {
            let mut memory = self.memory.borrow_mut();
            let memory = memory.as_mut().unwrap();
            let note = memory.memory.set.iter().find(|n| n.id == id).cloned();
            memory.memory.set.retain(|n| n.id != id);
            note
        };
        if let Some(note) = note.clone() {
            self.emit(SessionEvent::Memory(MemoryEvent::Forgot { scope: MemoryScope::Set, note }));
        }
        Ok(note)
    }
    fn has_library(&self) -> bool {
        self.library.borrow().is_some()
    }
    fn library(&self) -> Option<LibraryStatus> {
        self.library.borrow().clone()
    }
    fn has_taste(&self) -> bool {
        self.library.borrow().is_some()
    }
    async fn taste(&self) -> Result<Vec<KeptLine>, RuntimeError> {
        Ok(self.taste.borrow().clone())
    }
    fn has_forget_taste(&self) -> bool {
        true
    }
    async fn forget_taste(&self, id: &str) -> Result<bool, RuntimeError> {
        let mut taste = self.taste.borrow_mut();
        let old = taste.len();
        taste.retain(|l| l.id != id);
        Ok(taste.len() != old)
    }
    async fn conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        Ok(self.conversations.borrow().clone())
    }
    async fn resume_conversation(&self, id: &str) -> Result<bool, RuntimeError> {
        self.resumed.borrow_mut().push(id.into());
        Ok(true)
    }
    fn has_reconnect(&self) -> bool {
        self.reconnect.get()
    }
    async fn reconnect(&self) -> Result<(), RuntimeError> {
        self.calls.borrow_mut().push("reconnect".into());
        Ok(())
    }
    fn has_stop_live(&self) -> bool {
        self.stop.get()
    }
    async fn stop_live(&self) -> Result<bool, RuntimeError> {
        self.calls.borrow_mut().push("stopLive".into());
        Ok(true)
    }
}
struct Fixture {
    input: Rc<Input>,
    out: Rc<Out>,
    control: Rc<Control>,
    terminal: PlainTerminal,
    models: Rc<FakeModels>,
}
impl Fixture {
    fn new(tty: bool, hold: bool, notice: Option<&str>, updates: Option<UpdateControl>, models: Option<Rc<FakeModels>>) -> Self {
        Self::with(tty, hold, notice, updates, models, |_| {})
    }
    fn with(
        tty: bool,
        hold: bool,
        notice: Option<&str>,
        updates: Option<UpdateControl>,
        models: Option<Rc<FakeModels>>,
        configure: impl FnOnce(&mut TerminalOptions),
    ) -> Self {
        let input = Rc::new(Input { tty, ..Default::default() });
        let out = Rc::new(Out { tty, ..Default::default() });
        let control = Rc::new(Control::default());
        control.hold.set(hold);
        let models = models.unwrap_or_else(FakeModels::chosen);
        let mut options = TerminalOptions::new(control.clone(), input.clone(), out.clone(), models.clone(), "inference-only");
        options.startup_notice = notice.map(str::to_string);
        options.updates = updates;
        options.secrets = vec!["private-token".into()];
        options.close_timeout_ms = Some(25);
        configure(&mut options);
        let terminal = create_terminal(options);
        let emit = terminal.clone();
        *control.emit.borrow_mut() = Some(Rc::new(move |event| emit.handle_event(event)));
        drop(terminal.run());
        Self { input, out, control, terminal, models }
    }
    fn emit(&self, value: Value) {
        self.terminal.handle_event(serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{value}: {e}")));
    }
    fn text(&self) -> String {
        self.out.text.borrow().clone()
    }
    async fn quit(&self) -> i32 {
        let code = self.terminal.close().await;
        self.control.emit.borrow_mut().take();
        code
    }
}
async fn flush() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}
#[tokio::test]
async fn whole_event_output_matches_typescript_reference() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let reference: Value = serde_json::from_str(include_str!("support/terminal/reference.json")).unwrap();
            let f = Fixture::new(false, false, None, None, None);
            flush().await;
            assert_eq!(f.text(), reference["header"].as_str().unwrap());
            f.out.text.borrow_mut().clear();
            for case in reference["cases"].as_array().unwrap() {
                f.emit(case["event"].clone());
                assert_eq!(f.text(), case["expected"].as_str().unwrap(), "event: {}", case["event"]);
                f.out.text.borrow_mut().clear();
            }
            f.quit().await;
            assert_eq!(f.text(), reference["closed"].as_str().unwrap());
        })
        .await;
}
#[tokio::test]
async fn pipe_submission_starts_a_fresh_assistant_prefix() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = Fixture::new(false, false, None, None, None);
            flush().await;
            f.out.text.borrow_mut().clear();
            for line in ["first", "second"] {
                f.input.write(&format!("{line}\n"));
                flush().await;
                f.emit(json!({"type":"text", "text":format!("{line}\n")}));
            }
            assert_eq!(f.text(), "assistant> first\nassistant> second\n");
            f.quit().await;
        })
        .await;
}
#[tokio::test]
async fn header_transcript_tool_usage_dispatch_and_redaction() {
    tokio::task::LocalSet::new().run_until(async{let f=Fixture::new(false,false,None,None,None);flush().await;f.input.write("/help\n/status\n/refresh\n/new\n/unknown\nquestion\n");flush().await;for event in [json!({"type":"text","text":"hello "}),json!({"type":"text","text":"world"}),json!({"type":"tool-start","id":"t","name":"live_status"}),json!({"type":"tool-end","id":"t","name":"live_status","elapsedMs":7,"isError":false}),json!({"type":"retry","reason":"ChatGPT is busy (HTTP 429)","waitMs":4200}),json!({"type":"retry","reason":"ChatGPT's answer broke off","waitMs":0}),json!({"type":"turn-complete","elapsedMs":55,"result":{"stopReason":"completed","usage":{"inputTokens":3,"outputTokens":2,"cacheReadTokens":0,"cacheWriteTokens":0}}}),json!({"type":"error","message":"token=private-token\u{1b}[31m bad"})]{f.emit(event);}f.input.write("/quit\n");assert_eq!(f.quit().await,0);let text=f.text();for want in ["Kumi","continues next time","No Live access","hello world","live_status success · 7 ms","[wait] ChatGPT is busy (HTTP 429); trying again in 5 s","[wait] ChatGPT's answer broke off; carrying on","3/2"]{assert!(text.contains(want),"{text}");}assert_eq!(text.matches("hello world").count(),1);assert!(!text.contains("private-token"));assert_eq!(*f.control.calls.borrow(),["start","refresh","new","submit:question","close"]);}).await;
}
#[tokio::test]
async fn changes_undo_and_queued_connecting_request() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = Fixture::new(false, false, None, None, None);
            flush().await;
            f.emit(json!({"type":"change","change":{"id":"c1","family":"tempo","title":"Tempo 120 → 124 BPM","state":"applied","at":1}}));
            f.input.write("/undo\n");
            flush().await;
            assert!(f.text().contains("[change] Tempo 120 → 124 BPM (/undo takes it back)"));
            assert!(f.text().contains("[undo] Undid: Tempo 120 → 124 BPM"));
            f.control.state.set(TurnState::Running);
            f.input.write("early question\n");
            flush().await;
            assert!(!f.control.calls.borrow().contains(&"submit:early question".into()));
            assert!(f.text().contains("[waiting] Kumi is getting ready"));
            f.control.state.set(TurnState::Idle);
            f.emit(json!({"type":"state","state":"idle"}));
            flush().await;
            assert!(f.control.calls.borrow().contains(&"submit:early question".into()));
            f.quit().await;
        })
        .await;
}
#[tokio::test]
async fn startup_notice_follows_header_once() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = Fixture::new(false, false, Some("The Ableton bridge isn't installed yet"), None, None);
            flush().await;
            f.quit().await;
            let text = f.text();
            assert_eq!(text.matches("bridge isn't installed yet").count(), 1);
            assert!(text.find("/help for commands").unwrap() < text.find("bridge isn't installed yet").unwrap());
        })
        .await;
}
#[tokio::test]
async fn partial_wrapped_unicode_cursor_survives_streaming_and_notices() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = Fixture::new(true, false, None, None, None);
            flush().await;
            let prefix = "猫🎹".repeat(15);
            f.input.write(&format!("{prefix}abcd"));
            f.input.write("\x1b[D\x1b[D");
            f.emit(json!({"type":"text","text":"A streamed response"}));
            f.emit(json!({"type":"notice","message":"still working"}));
            f.emit(json!({"type":"tool-start","id":"x","name":"live_discover"}));
            f.input.write("XY\r");
            flush().await;
            assert!(f.control.calls.borrow().contains(&format!("submit:{prefix}abXYcd")));
            f.input.write("/quit\r");
            f.quit().await;
            assert!(!f.input.raw.get());
            assert!(kumi::text::sanitize_text(&f.text(), &[]).ends_with("Kumi closed. Each Set's conversation continues next time.\n"));
        })
        .await;
}
#[tokio::test]
async fn busy_rejection_cancel_preserves_input_and_close_is_once() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = Fixture::new(true, true, None, None, None);
            flush().await;
            f.input.write("first\r");
            flush().await;
            f.input.write("second\r/refresh\r/new\r");
            flush().await;
            assert_eq!(f.control.calls.borrow().iter().filter(|c| c.starts_with("submit:")).cloned().collect::<Vec<_>>(), ["submit:first"]);
            assert!(f.text().contains("Busy; cancel first"));
            f.input.write("follow\x03");
            flush().await;
            assert!(f.control.calls.borrow().contains(&"cancel".into()));
            assert!(!f.control.calls.borrow().contains(&"close".into()));
            f.input.write("up\r");
            flush().await;
            assert!(f.control.calls.borrow().contains(&"submit:followup".into()));
            f.input.write("\x03");
            flush().await;
            f.input.write("\x03");
            assert_eq!(f.quit().await, 0);
            assert_eq!(f.control.calls.borrow().iter().filter(|c| *c == "close").count(), 1);
        })
        .await;
}
#[tokio::test]
async fn eof_and_shutdown_deadline_suppress_late_output_restore_raw_mode() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = Fixture::new(false, true, None, None, None);
            flush().await;
            f.input.write("pending\n");
            flush().await;
            f.input.end();
            f.quit().await;
            let before = f.text();
            f.emit(json!({"type":"text","text":"late-secret"}));
            assert_eq!(f.text(), before);
            assert_eq!(f.control.calls.borrow().iter().filter(|c| *c == "close").count(), 1);
            let f = Fixture::new(true, false, None, None, None);
            flush().await;
            f.control.hang_close.set(true);
            f.input.write("/quit\r");
            assert_eq!(f.quit().await, 1);
            assert!(!f.input.raw.get());
            assert!(f.text().contains("Shutdown deadline"));
        })
        .await;
}
#[tokio::test]
async fn model_listing_choosing_effort_and_auth_advice() {
    tokio::task::LocalSet::new().run_until(async{let f=Fixture::new(false,false,None,None,Some(FakeModels::catalog()));flush().await;f.input.write("/model\n/model openai-codex\n/model openai-codex/gpt-6-luna\n/effort low\n/effort turbo\n/logout openai-codex\n");flush().await;f.emit(json!({"type":"error","message":"Not signed in to Anthropic: add its API key with /login (or set ANTHROPIC_API_KEY).","kind":"auth","provider":"anthropic"}));f.input.end();f.quit().await;let text=f.text();for want in ["Kumi · openai-codex/gpt-6-astra","[model] openai-codex/gpt-6-astra. List a provider's with /model <provider> (openai-codex)","[model] openai-codex: gpt-6-astra, gpt-6-luna","[model] openai-codex/gpt-6-luna from the next answer on.","[effort] low.","[effort] Choose one of low, medium, high, xhigh, max or default.","login anthropic"]{assert!(text.contains(want),"{text}");}assert_eq!(*f.models.calls.borrow(),["list:openai-codex","choose:openai-codex/gpt-6-luna","effort:low","signout:openai-codex"]);}).await;
}
#[tokio::test]
async fn memory_and_library_learning_taste_are_listed_and_forgotten() {
    tokio::task::LocalSet::new().run_until(async{let f=Fixture::new(false,false,None,None,None);flush().await;*f.control.memory.borrow_mut()=Some(serde_json::from_value(json!({"producer":[{"id":"p1","text":"Prefers short reverbs","at":1}],"set":[{"id":"s1","text":"The Reese is the main bass","at":2}],"setName":"Night Drive","saved":true})).unwrap());f.input.write("/note p1 Prefers short, dark reverbs\n/pin p1\n/note p9 x\n/memory\n/forget s1\n/forget s9\n");flush().await;f.emit(json!({"type":"remembered","scope":"producer","note":{"id":"p2","text":"Names buses BUS - <what>","at":3}}));for want in ["[memory] Changed note p1.","[memory] Pinned p1: Kumi keeps it even when its memory is full.","[memory] Use: /note <id> <new words>","[memory] About you: p1 (pinned) Prefers short, dark reverbs","[memory] About Night Drive: s1 The Reese is the main bass","[memory] Forgot: The Reese is the main bass","[memory] Use: /forget <id>","[memory] Noted about you: Names buses BUS - <what>"]{assert!(f.text().contains(want));}
 let status:LibraryStatus=serde_json::from_value(json!({"state":"learning","sounds":10,"presets":2,"sets":1,"todo":50,"done":10})).unwrap();*f.control.library.borrow_mut()=Some(status.clone());*f.control.taste.borrow_mut()=vec![KeptLine{id:"tempo".into(),line:"Tempo: usually 124 BPM".into()},KeptLine{id:"chain-vocal".into(),line:"Vocals: EQ Eight → Compressor".into()}];for _ in 0..3{f.terminal.handle_event(SessionEvent::Library(LibraryEvent{status:status.clone()}));}f.input.write("/status\n/memory\n/forget u2\n/forget u9\n");flush().await;let text=f.text();assert_eq!(text.matches("Learning your library in the background…").count(),1);for want in ["; Learning your library in the background · 10 of 50 sounds","[memory] From your Sets: u1 Tempo: usually 124 BPM · u2 Vocals: EQ Eight → Compressor","[memory] Forgot, from your Sets: Vocals: EQ Eight → Compressor"]{assert!(text.contains(want),"{text}");}f.quit().await;}).await;
}
#[tokio::test]
async fn watched_video_and_web_events_have_original_summary_words() {
    tokio::task::LocalSet::new().run_until(async{let f=Fixture::new(false,false,None,None,None);flush().await;f.emit(json!({"type":"watched","title":"1 Minute Reese With Operator","url":"https://youtu.be/x","duration":81,"from":0,"to":81,"chapters":[],"words":"automatic","lines":6,"frames":[{"at":5,"thumb":{"width":32,"height":18,"rgb":vec![0;32*18*3]}},{"at":65,"thumb":{"width":32,"height":18,"rgb":vec![0;32*18*3]}}],"sound":{"from":20,"to":30},"notes":["Kumi couldn't take the frame at 1:10 (private-token)."]}));for event in [json!({"type":"web","action":"searched","title":"erbe verb","where":"github","via":"GitHub","results":1}),json!({"type":"web","action":"read","title":"https://example.com/manual","url":"https://example.com/manual","kind":"a page"}),json!({"type":"web","action":"searched","title":"nothing like this private-token","where":"web","via":"Exa","results":0})]{f.emit(event);}f.quit().await;let text=f.text();for want in ["[watched] “1 Minute Reese With Operator” (1:21): 0:00–1:21, its automatic captions, frames at 0:05, 1:05, the sound at 0:20–0:30","[watched] Kumi couldn't take the frame at 1:10","[web] Searched GitHub for “erbe verb” · 1 repository","[web] Read example.com/manual\n","nothing found"]{assert!(text.contains(want),"{text}");}assert!(!text.contains("private-token"));}).await;
}
#[tokio::test]
async fn update_check_offer_and_request_close() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let requested = Rc::new(Cell::new(0));
            let latest = Rc::new(RefCell::new(None));
            let asked = latest.clone();
            let request = requested.clone();
            let updates = UpdateControl {
                current: "1.0.0".into(),
                check: Rc::new(move || {
                    let value = asked.borrow().clone();
                    async move { Ok(value) }.boxed_local()
                }),
                request: Rc::new(move || request.set(request.get() + 1)),
            };
            let f = Fixture::new(false, false, None, Some(updates), None);
            flush().await;
            f.input.write("/update\n");
            flush().await;
            assert!(f.text().contains("[update] Kumi is up to date (1.0.0)."));
            *latest.borrow_mut() = Some("1.1.0".into());
            f.terminal.offer_update("1.1.0");
            assert!(f.text().contains("[update] Kumi 1.1.0 is out (this is 1.0.0). /update gets it."));
            f.input.write("/update\n");
            assert_eq!(f.quit().await, 0);
            assert_eq!(requested.get(), 1);
            assert!(f.text().contains("Updating to Kumi 1.1.0: Kumi closes, updates and opens again."));
        })
        .await;
}
#[tokio::test]
async fn willington_is_said_off_at_the_start_and_switched_by_its_command() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let on = Rc::new(Cell::new(false));
            let willington = WillingtonControl {
                on: {
                    let on = on.clone();
                    Rc::new(move || Some(on.get()))
                },
                set: {
                    let on = on.clone();
                    Rc::new(move |value| {
                        on.set(value);
                        async { Ok(()) }.boxed_local()
                    })
                },
            };
            let f = Fixture::with(false, false, None, None, None, |o| o.willington = Some(willington));
            flush().await;
            assert!(f.text().contains("[willington] Willington bindings are OFF currently, type /willington to toggle them on"));
            f.input.write("/willington\n");
            flush().await;
            assert!(on.get());
            assert!(f.text().contains("[willington] Willington bindings are ON: Kumi can map rack macros"));
            f.input.write("/willington\n/help\n");
            flush().await;
            assert!(!on.get());
            let text = f.text();
            assert!(text.contains("[willington] Willington bindings are OFF. /willington turns them on again."), "{text}");
            assert!(text.contains("/willington (Willington's bindings on or off)"), "{text}");
            f.quit().await;
            // Without Willington in the bridge: nothing at the start, and no such command.
            let f = Fixture::new(false, false, None, None, None);
            flush().await;
            f.input.write("/willington\n");
            flush().await;
            assert!(!f.text().contains("[willington]"));
            assert!(f.text().contains("Unknown command. Use /help."));
            f.quit().await;
        })
        .await;
}
#[tokio::test]
async fn conversations_resume_reconnect_and_new() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = Fixture::new(false, false, None, None, None);
            flush().await;
            let now = kumi_common::time::now_ms();
            *f.control.conversations.borrow_mut() = vec![
                ConversationSummary { id: "now001".into(), saved_at: now, first: "add a hi-hat groove".into(), turns: 2, current: true },
                ConversationSummary {
                    id: "old001".into(),
                    saved_at: now - 7200000,
                    first: "make the bass wider".into(),
                    turns: 5,
                    current: false,
                },
            ];
            f.control.reconnect.set(true);
            f.input.write("/conversations\n");
            flush().await;
            assert!(f.text().contains("1. add a hi-hat groove (this one, 2 requests) · 2. make the bass wider (2 hours ago, 5 requests)"));
            f.input.write("/conversations 2\n");
            flush().await;
            assert_eq!(*f.control.resumed.borrow(), ["old001"]);
            f.emit(json!({"type":"resumed","savedAt":now-7200000,"chosen":true,"lines":[{"role":"user","text":"make the bass wider"}]}));
            assert!(f.text().contains("Back to your conversation from 2 hours ago"));
            assert!(f.text().contains("you> make the bass wider"));
            f.input.write("/reconnect\n/new\n");
            flush().await;
            assert!(f.control.calls.borrow().contains(&"reconnect".into()));
            assert!(f.control.calls.borrow().contains(&"new".into()));
            f.quit().await;
        })
        .await;
}
#[tokio::test]
async fn output_bound_cancels_and_eof_dispatches_last_line() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = Fixture::new(false, true, None, None, None);
            flush().await;
            f.input.write("pending\n");
            f.emit(json!({"type":"text","text":"x".repeat(256*1024+1)}));
            flush().await;
            assert!(f.control.calls.borrow().contains(&"cancel".into()));
            assert!(f.text().contains("terminal bound"));
            f.quit().await;
            let f = Fixture::new(false, false, None, None, None);
            flush().await;
            f.input.write("last line");
            f.input.end();
            f.quit().await;
            assert!(f.control.calls.borrow().contains(&"submit:last line".into()));
        })
        .await;
}
