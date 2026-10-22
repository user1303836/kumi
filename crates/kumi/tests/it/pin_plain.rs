//! The plain terminal's whole output as Kumi 1.18.0 writes it for Live, for the same scenes as the full-screen
//! app's goldens (pin_screens.rs): from its first line to its last, kept as text in tests/goldens/tui/plain;
//! `KUMI_BLESS=1` writes them again. Times are given relative to now (saved 3 hours ago), so the words are the
//! same at any moment.
#[path = "../support/models.rs"]
mod models;
use async_trait::async_trait;
use kumi::{
    input::{ByteListener, TerminalInput},
    terminal::{create_terminal, PlainTerminal, Terminal, TerminalOptions},
    tui::tty::TtyOutput,
};
use kumi_runtime::core::{contracts::*, errors::RuntimeError};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const HOUR: i64 = 3_600_000;

fn golden(name: &str, actual: &str) {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/tui/plain").join(name);
    // Nothing of this machine's: no CR, no home or temp folder.
    assert!(!actual.contains('\r'), "{name} has a CR in it");
    let home = |key: &str| std::path::PathBuf::from(std::env::var_os(key).unwrap_or_default());
    for folder in [std::env::temp_dir(), home("HOME"), home("USERPROFILE")] {
        let folder = folder.display().to_string();
        assert!(folder.len() < 2 || !actual.contains(folder.trim_end_matches(['/', '\\'])), "{name} names {folder}");
    }
    if std::env::var_os("KUMI_BLESS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&file)
        .unwrap_or_else(|_| panic!("{} is missing: run this test with KUMI_BLESS=1", file.display()))
        .replace("\r\n", "\n");
    if actual != expected {
        panic!("{} changed: check the difference, then run this test with KUMI_BLESS=1\n{}", file.display(), difference(&expected, actual));
    }
}

/// Where `actual` first parts from `expected`, with the lines around it: `-` expected, `+` actual, by line number.
fn difference(expected: &str, actual: &str) -> String {
    let (expected, actual): (Vec<&str>, Vec<&str>) = (expected.split('\n').collect(), actual.split('\n').collect());
    let rows = expected.len().max(actual.len());
    let differ: Vec<usize> = (0..rows).filter(|&row| expected.get(row) != actual.get(row)).collect();
    let first = differ.first().copied().unwrap_or(0);
    let mut out = vec![format!(
        "{} differing lines, first at line {}: {}",
        differ.len(),
        first + 1,
        differ.iter().take(12).map(|row| (row + 1).to_string()).collect::<Vec<_>>().join(", ")
    )];
    for row in first.saturating_sub(3)..(first + 4).min(rows) {
        match (expected.get(row), actual.get(row)) {
            (Some(e), Some(a)) if e == a => out.push(format!("  {:>4}  {e}", row + 1)),
            (e, a) => {
                out.push(format!("- {:>4}  {}", row + 1, e.copied().unwrap_or("(no line)")));
                out.push(format!("+ {:>4}  {}", row + 1, a.copied().unwrap_or("(no line)")));
            }
        }
    }
    out.join("\n")
}

#[derive(Default)]
struct Input {
    data: RefCell<Option<ByteListener>>,
    end: RefCell<Option<Rc<dyn Fn()>>>,
}
impl TerminalInput for Input {
    fn is_tty(&self) -> bool {
        false
    }
    fn is_raw(&self) -> bool {
        false
    }
    fn set_raw_mode(&self, _: bool) -> std::io::Result<()> {
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

#[derive(Default)]
struct Out {
    text: RefCell<String>,
}
impl TtyOutput for Out {
    fn is_tty(&self) -> bool {
        false
    }
    fn columns(&self) -> Option<i32> {
        Some(120)
    }
    fn rows(&self) -> Option<i32> {
        Some(36)
    }
    fn write(&self, s: &str) {
        self.text.borrow_mut().push_str(s);
    }
}

/// A session that answers what each scene asks of it.
struct Control {
    state: Cell<TurnState>,
    memory: RefCell<Option<MemoryView>>,
    techniques: RefCell<Vec<TechniqueSummary>>,
    taste: RefCell<Vec<KeptLine>>,
    library: RefCell<Option<LibraryStatus>>,
    recipes: RefCell<Vec<RecipeSummary>>,
    conversations: RefCell<Vec<ConversationSummary>>,
    all: RefCell<Vec<ConversationSummary>>,
    now: RefCell<Option<ConversationNow>>,
    stop: Cell<bool>,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            state: Cell::new(TurnState::Idle),
            memory: RefCell::default(),
            techniques: RefCell::default(),
            taste: RefCell::default(),
            library: RefCell::default(),
            recipes: RefCell::default(),
            conversations: RefCell::default(),
            all: RefCell::default(),
            now: RefCell::default(),
            stop: Cell::new(false),
        }
    }
}
#[async_trait(?Send)]
impl SessionController for Control {
    async fn start(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn submit(&self, _: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        self.state.set(TurnState::Idle);
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        self.state.set(TurnState::Closed);
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        SessionStatus { state: self.state.get(), connection: ConnectionState::Connected, turns: 0, max_turns: Some(30), observation: None }
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
    fn has_memory(&self) -> bool {
        self.memory.borrow().is_some()
    }
    async fn memory(&self) -> Result<Option<MemoryView>, RuntimeError> {
        Ok(self.memory.borrow().clone())
    }
    fn has_techniques(&self) -> bool {
        self.memory.borrow().is_some()
    }
    async fn techniques(&self) -> Result<Vec<TechniqueSummary>, RuntimeError> {
        Ok(self.techniques.borrow().clone())
    }
    fn has_taste(&self) -> bool {
        self.library.borrow().is_some()
    }
    async fn taste(&self) -> Result<Vec<KeptLine>, RuntimeError> {
        Ok(self.taste.borrow().clone())
    }
    fn has_library(&self) -> bool {
        self.library.borrow().is_some()
    }
    fn library(&self) -> Option<LibraryStatus> {
        self.library.borrow().clone()
    }
    fn has_recipes(&self) -> bool {
        true
    }
    async fn recipes(&self) -> Result<Vec<RecipeSummary>, RuntimeError> {
        Ok(self.recipes.borrow().clone())
    }
    fn has_conversations(&self) -> bool {
        true
    }
    async fn conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        Ok(self.conversations.borrow().clone())
    }
    async fn all_conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        Ok(self.all.borrow().clone())
    }
    async fn conversation_now(&self) -> Option<ConversationNow> {
        self.now.borrow().clone()
    }
    fn has_stop_live(&self) -> bool {
        self.stop.get()
    }
    async fn stop_live(&self) -> Result<bool, RuntimeError> {
        Ok(true)
    }
}

struct Fixture {
    input: Rc<Input>,
    out: Rc<Out>,
    control: Rc<Control>,
    terminal: PlainTerminal,
}
impl Fixture {
    async fn new(control: Control) -> Self {
        let input = Rc::new(Input::default());
        let out = Rc::new(Out::default());
        let control = Rc::new(control);
        let mut options = TerminalOptions::new(control.clone(), input.clone(), out.clone(), models::FakeModels::chosen(), "live");
        options.secrets = vec!["private-token".into()];
        options.close_timeout_ms = Some(25);
        let terminal = create_terminal(options);
        drop(terminal.run());
        let f = Self { input, out, control, terminal };
        flush().await;
        f
    }
    /// The session's event, as it sends it; a turn's state goes to the session's status too.
    fn emit(&self, value: Value) {
        let event: SessionEvent = serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{value}: {e}"));
        if let SessionEvent::State { state } = event {
            self.control.state.set(state);
        }
        self.terminal.handle_event(event);
    }
    fn connect(&self) {
        self.emit(json!({"type":"connection","state":"connected"}));
        self.emit(json!({"type":"observation","label":"Current open Set: Night Drive — Remote Script · real-live"}));
    }
    async fn line(&self, text: &str) {
        let data = self.input.data.borrow().clone();
        if let Some(data) = data {
            data(format!("{text}\n").as_bytes());
        }
        flush().await;
    }
    /// Closes the terminal and keeps all it wrote.
    async fn pin(self, name: &str) {
        flush().await;
        self.terminal.close().await;
        golden(&format!("{name}.txt"), &self.out.text.borrow());
    }
}

async fn flush() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

fn now_ms() -> i64 {
    kumi_common::time::now_ms()
}

macro_rules! scene {
    ($name:ident, $body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

fn change(id: &str, family: &str, title: &str, state: &str, extra: Value) -> Value {
    let mut change = json!({"id":id,"family":family,"title":title,"state":state,"at":1});
    for (key, value) in extra.as_object().unwrap() {
        change[key] = value.clone();
    }
    json!({"type":"change","change":change})
}

scene!(start, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    f.pin("start").await;
});

scene!(focus, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    // FOCUS, pins and the beat have no words here.
    f.emit(json!({"type":"focus","focus":{"track":{"name":"Bass","color":"#f59a3c","kind":"midi"},"trackRef":"3:track:1","device":"Operator","detail":"Device","view":"Session","parameter":{"name":"Filter Freq","value":"1.20 kHz","owner":"Operator"}}}));
    f.emit(json!({"type":"focus","focus":{"track":{"name":"Keys","color":"#5ec1f7","kind":"midi"},"slotRef":"3:clip_slot:2:0","clip":"Chords","detail":"Clip","view":"Session","selectedNotes":2}}));
    f.emit(json!({"type":"focus","focus":{"track":{"name":"Bass","color":"#f59a3c","kind":"midi"},"trackRef":"3:track:1","view":"Arrangement"}}));
    f.emit(json!({"type":"pointed","pin":{"trackRef":"3:track:1","ref":"3:track:1","node":"selection","name":"Bass","trail":[],"siblings":["Drums"],"live":true,"track":"Bass","time":{"fromBeat":64,"toBeat":80}}}));
    f.emit(json!({"type":"transport","transport":{"playing":true,"tempo":123.5,"beat":8,"at":0,"beatsPerBar":4}}));
    f.pin("focus").await;
});

scene!(changes, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    let value = |name: &str, from: f64, to: f64| json!({"name":name,"from":from,"to":to});
    let band = |gain: f64| json!({"shape":"bell","freq":1000.0,"gainDb":gain,"q":1.0});
    let note = |start: f64, pitch: f64| json!([start, pitch, 1.0, 100.0]);
    for change in [
        change(
            "c1",
            "parameter",
            "Operator · Filter Freq 1.20 kHz → 800 Hz",
            "applied",
            json!({"picture":{"kind":"values","values":[value("Filter Freq",0.62,0.55)]}}),
        ),
        change(
            "c2",
            "parameter",
            "EQ Eight · 3 Gain A 0.00 dB → -4.00 dB",
            "applied",
            json!({"picture":{"kind":"eq","before":[band(0.0)],"after":[band(-4.0)]}}),
        ),
        change(
            "c3",
            "parameter",
            "Compressor · Ratio 2.00 : 1 → 4.00 : 1",
            "applied",
            json!({"picture":{"kind":"dynamics","before":{"thresholdDb":-12.0,"ratio":2.0},"after":{"thresholdDb":-12.0,"ratio":4.0}}}),
        ),
        change(
            "c4",
            "parameter",
            "Filter Freq automation, bars 1–5",
            "applied",
            json!({"picture":{"kind":"automation","min":0.0,"max":1.0,"before":[[0.0,0.2]],"after":[[0.0,0.2],[8.0,0.8]]}}),
        ),
        change(
            "c5",
            "mixer",
            "Bass volume 0.0 dB → -3.0 dB",
            "applied",
            json!({"picture":{"kind":"mix","before":{"volume":0.85},"after":{"volume":0.7}}}),
        ),
        change(
            "c6",
            "clip",
            "Chords · 1 note added",
            "applied",
            json!({"picture":{"kind":"notes","length":4.0,"before":[note(0.,60.)],"after":[note(0.,60.),note(1.,64.)]}}),
        ),
        change(
            "c7",
            "clip",
            "New MIDI clip “Chord” · 3 notes",
            "applied",
            json!({"clip":{"length":4.0,"notes":[{"pitch":60.0,"start":0.0,"duration":1.0,"velocity":100.0}]}}),
        ),
        change(
            "c8",
            "device",
            "Loaded Saturator into Audio Effect Rack › Chain 1",
            "applied",
            json!({"devices":{"rack":"Audio Effect Rack","chains":[{"name":"Chain 1","devices":["Saturator"]}],"chain":0,"index":0}}),
        ),
        change("c9", "color", "Bass colour orange → blue", "applied", json!({"colors":{"from":"#f59a3c","to":"#5ec1f7"}})),
        change("c10", "mixer", "Bass volume down", "applied", json!({"from":0.85,"to":0.6,"range":[0.0,1.0]})),
    ] {
        f.emit(change);
    }
    f.pin("changes").await;
});

scene!(change_states, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    for change in [
        change("c1", "tempo", "Tempo 120 → 124 BPM", "applied", json!({})),
        change("c1", "tempo", "Tempo 120 → 124 BPM", "applied", json!({"from":120.0,"to":124.0})),
        change("c2", "rename", "Renamed 2-MIDI to Lead", "undone", json!({})),
        change("c3", "structure", "Added return track “Drum Bus”", "kept", json!({"note":"Live's own undo can take it back."})),
        change("c4", "device", "Loaded Glue Compressor on Drums", "unsure", json!({"note":"Live didn't confirm it."})),
        change("c5", "device", "Loaded Operator on Bass", "unsure", json!({})),
        change("c6", "clip", "New MIDI clip “Hook”", "expired", json!({"note":"Live opened another Set."})),
        change("c7", "clip", "New MIDI clip “Verse”", "expired", json!({"note":"Live opened another Set."})),
        change("c8", "parameter", "Tried 3 pads against the reference", "heard", json!({"score":82.0})),
        change("c9", "parameter", "Listened to the drop", "heard", json!({})),
    ] {
        f.emit(change);
    }
    f.pin("change-states").await;
});

scene!(set_asked, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    f.emit(json!({"type":"set-asked","ask":{"id":"far001","title":"Bass ideas","set":"Morning Run","open":"Night Drive","daw":"Live","noun":"Set"}}));
    f.emit(json!({"type":"set-asked","ask":{"id":"far002","title":"","daw":"Live","noun":"Set"}}));
    f.emit(json!({"type":"set-asked"}));
    f.pin("set-asked").await;
});

fn conversations() -> Control {
    let now = now_ms();
    let control = Control::default();
    let row = |id: &str, saved: i64, first: &str, turns: u32, current: bool, extra: Value| {
        let mut row = json!({"id":id,"savedAt":saved,"first":first,"turns":turns,"current":current,"place":"a"});
        for (key, value) in extra.as_object().unwrap() {
            row[key] = value.clone();
        }
        row
    };
    let here = vec![
        row("now001", now, "add a hi-hat groove", 2, true, json!({"set":"Night Drive"})),
        row("old001", now - 2 * HOUR, "make the bass wider", 5, false, json!({"set":"Night Drive"})),
    ];
    let mut all = here.clone();
    all.push(row(
        "far001",
        now - 3 * HOUR,
        "sketch a bassline",
        4,
        false,
        json!({"place":"b","set":"Morning Run","pinned":true,"name":"Bass ideas"}),
    ));
    all.push(row("far002", now - 26 * HOUR, "sketch some chords", 1, false, json!({"place":"unsaved"})));
    *control.conversations.borrow_mut() = serde_json::from_value(json!(here)).unwrap();
    *control.all.borrow_mut() = serde_json::from_value(json!(all)).unwrap();
    control
}

scene!(conversations_list, async {
    let f = Fixture::new(conversations()).await;
    f.connect();
    f.line("/resume").await;
    f.pin("conversations-list").await;
});

fn memory(saved: bool) -> Control {
    let now = now_ms();
    let control = Control::default();
    *control.memory.borrow_mut() = Some(
        serde_json::from_value(json!({
            "producer":[{"id":"p1","text":"Prefers short, dark reverbs","at":now-2*HOUR},{"id":"p2","text":"Mixes at -14 LUFS","at":now-26*HOUR,"pinned":true}],
            "set":[{"id":"s1","text":"The Reese is the main bass","at":now-HOUR}],
            "setName":"Night Drive","saved":saved}))
        .unwrap(),
    );
    *control.techniques.borrow_mut() = serde_json::from_value(
        json!([{"id":"t1","name":"Neuro from a Reese","fits":"gritty, moving neuro basses","source":"Au5 · Neuro bass"}]),
    )
    .unwrap();
    *control.taste.borrow_mut() = serde_json::from_value(json!([{"id":"chain-vocal","line":"Vocals: EQ Eight → Compressor → Reverb (on 4 of 4 vocal tracks)"},{"id":"tempo","line":"Tempo: usually 124–126 BPM"}])).unwrap();
    *control.library.borrow_mut() =
        Some(serde_json::from_value(json!({"state":"ready","sounds":1020,"presets":40,"sets":3,"learnedAt":1})).unwrap());
    *control.recipes.borrow_mut() = serde_json::from_value(json!([
        {"name":"Drum bus","about":"Glue, saturation and a short room on a new return","params":[],"steps":3,"used":2,"lastUsed":now-26*HOUR,"created":1},
        {"name":"Resample twice","about":"OTT and Saturator, then Grain Delay","params":[{"name":"track","about":"the track to resample"}],"steps":6,"used":0,"created":2}
    ]))
    .unwrap();
    control
}

scene!(memory_and_recipes, async {
    let f = Fixture::new(memory(true)).await;
    f.connect();
    f.line("/memory").await;
    f.line("/recipes").await;
    f.pin("memory-and-recipes").await;
});

scene!(memory_unsaved_set, async {
    let f = Fixture::new(memory(false)).await;
    f.connect();
    f.line("/memory").await;
    f.pin("memory-unsaved-set").await;
});

scene!(help_and_status, async {
    let control = memory(true);
    *control.now.borrow_mut() =
        Some(serde_json::from_value(json!({"id":"now001","title":"Bass ideas","kept":true,"touched":true})).unwrap());
    let f = Fixture::new(control).await;
    f.connect();
    f.line("/help").await;
    f.line("/status").await;
    f.pin("help-and-status").await;
});

scene!(stop_without_live, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    f.line("/stop").await;
    f.control.stop.set(true);
    f.emit(json!({"type":"connection","state":"disconnected"}));
    f.line("/stop").await;
    f.pin("stop-without-live").await;
});

scene!(catch_up, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    let at = now_ms() - 3 * HOUR;
    f.emit(json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":at,"lines":["Tempo 120 → 124 BPM","Added track “Pad”","Renamed 2-MIDI to Lead"],"more":2}}));
    f.emit(json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":at,"lines":[],"more":0}}));
    f.emit(
        json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":at,"lines":["Muted Drums"],"more":0,"afterReconnect":true}}),
    );
    f.pin("catch-up").await;
});

fn resumed(extra: Value) -> Value {
    let mut resumed = json!({"type":"resumed","savedAt":now_ms()-3*HOUR,
        "lines":[
            {"role":"user","text":"make the bass wider"},
            {"role":"assistant","text":"Widened it to 140%, keeping the sub mono.","tools":["live_discover","make_changes"]},
            {"role":"user","text":"and a bit darker, private-token"},
            {"role":"assistant","text":"Lowered the filter\nto 800 Hz."}],
        "changes":[{"id":"old001:c4","family":"mixer","title":"Bass width 100% → 140%","state":"expired","note":"From an earlier session, so Kumi can't undo it now.","at":1}]});
    for (key, value) in extra.as_object().unwrap() {
        resumed[key] = value.clone();
    }
    resumed
}

scene!(resumed_conversations, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    f.emit(resumed(json!({})));
    f.emit(resumed(json!({"chosen":true})));
    f.emit(resumed(json!({"unreadable":true})));
    f.pin("resumed-conversations").await;
});

/// The session's words for each cause, idle and working (core/session.rs).
const LOST: [(&str, &str, &str); 6] = [
    (
        "live",
        "Live closed. Kumi will pick up where you left off when it's back.",
        "Live closed while Kumi was working in it. If it crashed, your unsaved changes may be lost: Live offers to recover them when it opens. Kumi picks up when Live is back.",
    ),
    (
        "bridge",
        "Kumi's link to Live dropped. It's reconnecting, and will pick up where you left off.",
        "Kumi's link to Live dropped. It's reconnecting, and will pick up where you left off.",
    ),
    (
        "set",
        "Live is opening another Set; Kumi picks up once it's open.",
        "Live is opening another Set, so Kumi stopped what it was doing: nothing it was doing lands in the other Set. It picks up once the Set is open.",
    ),
    ("askedset", "Live is opening the Set; Kumi carries on once it's open.", "Live is opening the Set; Kumi carries on once it's open."),
    (
        "busy",
        "Live is busy and answering slowly; Kumi carries on once it answers.",
        "Live is busy and answering slowly, so Kumi is waiting for it: your request carries on once it answers.",
    ),
    (
        "none",
        "Kumi lost touch with Live. It will pick up where you left off when Live is back.",
        "Kumi lost touch with Live. It will pick up where you left off when Live is back.",
    ),
];
const STILL_MISSING: &str =
    "Kumi can't reach Live. Is it open, with AbletonMcpBridge chosen as a Control Surface (Settings → Link, Tempo & MIDI)?";

fn working(f: &Fixture) {
    f.emit(json!({"type":"state","state":"running"}));
    f.emit(json!({"type":"tool-start","id":"t1","name":"live_discover"}));
}

fn lost(f: &Fixture, cause: &str, running: bool) {
    let (_, idle, busy) = LOST.iter().find(|(name, _, _)| *name == cause).unwrap();
    f.emit(json!({"type":"connection","state":"disconnected"}));
    f.emit(json!({"type":"notice","message":if running { busy } else { idle }}));
}

fn stopped(f: &Fixture) {
    f.emit(json!({"type":"turn-complete","result":{"stopReason":"cancelled"},"elapsedMs":1200}));
    f.emit(json!({"type":"state","state":"idle"}));
}

fn back(f: &Fixture, set: &str, notices: &[&str]) {
    f.emit(json!({"type":"connection","state":"connected"}));
    f.emit(json!({"type":"observation","label":format!("Current open Set: {set} — Remote Script · real-live")}));
    for notice in notices {
        f.emit(json!({"type":"notice","message":notice}));
    }
}

macro_rules! disconnect {
    ($idle:ident, $running:ident, $cause:literal) => {
        scene!($idle, async {
            let f = Fixture::new(Control::default()).await;
            f.connect();
            lost(&f, $cause, false);
            f.pin(&stringify!($idle).replace('_', "-")).await;
        });
        scene!($running, async {
            let f = Fixture::new(Control::default()).await;
            f.connect();
            working(&f);
            lost(&f, $cause, true);
            f.pin(&stringify!($running).replace('_', "-")).await;
        });
    };
}
disconnect!(disconnect_live_idle, disconnect_live_running, "live");
disconnect!(disconnect_bridge_idle, disconnect_bridge_running, "bridge");
disconnect!(disconnect_set_idle, disconnect_set_running, "set");
disconnect!(disconnect_askedset_idle, disconnect_askedset_running, "askedset");
disconnect!(disconnect_busy_idle, disconnect_busy_running, "busy");
disconnect!(disconnect_none_idle, disconnect_none_running, "none");

scene!(disconnect_still_missing, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    lost(&f, "none", false);
    f.emit(json!({"type":"notice","message":STILL_MISSING}));
    f.pin("disconnect-still-missing").await;
});

scene!(connecting, async {
    let f = Fixture::new(Control::default()).await;
    f.emit(json!({"type":"connection","state":"connecting"}));
    f.pin("connecting").await;
});

scene!(reconnects, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    lost(&f, "live", false);
    back(&f, "Night Drive", &["Live is back."]);
    working(&f);
    lost(&f, "bridge", true);
    stopped(&f);
    back(&f, "Night Drive", &["Live is back. Your last request was stopped; press enter to send it again."]);
    f.emit(json!({"type":"resend","text":"make the kick punchier"}));
    working(&f);
    lost(&f, "live", true);
    stopped(&f);
    back(&f, "Night Drive", &["Live is back. Your last request stopped when Live closed, which it may have caused: check the Set before you send it again (enter sends it)."]);
    lost(&f, "set", false);
    back(&f, "Morning Run", &["Live has the other Set open."]);
    working(&f);
    lost(&f, "busy", true);
    back(&f, "Morning Run", &["Live is answering again."]);
    lost(&f, "busy", true);
    back(
        &f,
        "Night Drive",
        &[
            "Live opened another Set while it was busy, so Kumi stopped what it was doing: nothing it was doing lands in the other Set.",
            "Live has the other Set open.",
        ],
    );
    stopped(&f);
    f.pin("reconnects").await;
});

fn job(id: u32, state: &str, extra: Value) -> Value {
    let mut job = json!({"type":"job","id":id,"kind":"worker","title":"Burial vocal chops","state":state,"whenDone":"tell","startedAt":now_ms()-65_020,"steps":0,"sources":0});
    for (key, value) in extra.as_object().unwrap() {
        job[key] = value.clone();
    }
    job
}

scene!(jobs, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    let ended = now_ms();
    f.emit(job(1, "running", json!({"title":"Tape loop tutorial","kind":"tool","tool":"watch_video","doing":"starting"})));
    f.emit(job(
        1,
        "running",
        json!({"title":"Tape loop tutorial","kind":"tool","tool":"watch_video","doing":"watching a video","steps":1}),
    ));
    f.emit(job(2, "running", json!({"doing":"starting"})));
    f.emit(job(
        2,
        "done",
        json!({"line":"Burial pitches vocal snippets up and drowns them in reverb.","steps":5,"sources":3,"endedAt":ended}),
    ));
    f.emit(job(3, "failed", json!({"title":"Manual: Grain Delay","line":"The manual couldn't be reached.","endedAt":ended})));
    f.emit(job(4, "stopped", json!({"title":"Resample chain","endedAt":ended})));
    f.emit(json!({"type":"carry-on","job":2,"title":"Burial vocal chops"}));
    f.pin("jobs").await;
});

scene!(goal_match_and_loop, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    f.emit(json!({"type":"state","state":"running"}));
    f.emit(json!({"type":"goal","state":"running","goal":"make my pad sound like ~/ref.wav","generation":12,"rendered":36,"trend":[58,81],"first":58,"best":{"label":"Collision","score":81},"elapsedMs":185_020,"candidates":3}));
    f.emit(json!({"type":"goal","state":"done","goal":"make my pad sound like ~/ref.wav","generation":13,"rendered":39,"trend":[58,81],"first":58,"best":{"label":"Collision","score":81},"elapsedMs":200_000,"candidates":3,"bestTrack":"Kumi · Goal best","why":"stopped"}));
    f.emit(json!({"type":"objective","objective":"master this to -9 LUFS","state":"running","turns":1,"turnBudget":12,"elapsedMs":65_020,"budgetMs":3_600_000,"verdict":"continue","reason":"not met yet","next":"raise the limiter","measured":false}));
    f.emit(json!({"type":"judged","round":2,"kind":"judged","heard":"the mix, bars 49–57","target":"True peak","change":"Limiter ceiling -1 → -3.5 dB","rows":[],"kept":false,"why":"it hurt punch (crest)","met":false,"listens":4,"elapsedMs":42000}));
    f.emit(json!({"type":"loop","state":"running","request":"master this to -9 LUFS","rounds":2,"kept":1,"reverted":1,"listens":7,"elapsedMs":42_020,"roundsLeft":14,"next":"True peak"}));
    f.emit(json!({"type":"loop","state":"done","request":"master this to -9 LUFS","rounds":2,"kept":1,"reverted":1,"listens":5,"elapsedMs":61000,"roundsLeft":14,"stop":"met"}));
    f.emit(json!({"type":"objective","objective":"master this to -9 LUFS","state":"done","turns":2,"turnBudget":12,"elapsedMs":90000,"budgetMs":3_600_000,"verdict":"complete","reason":"the judge's checklist is met","measured":true}));
    f.emit(json!({"type":"match","state":"running","check":3,"first":58,"best":{"label":"Low shelf +2 dB","score":74},"elapsedMs":42_020,"roundsLeft":5}));
    f.emit(json!({"type":"match","state":"done","check":6,"first":58,"best":{"label":"Low shelf +3 dB","score":81},"elapsedMs":65_000,"roundsLeft":0,"stop":"reached"}));
    f.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":90000}));
    f.emit(json!({"type":"state","state":"idle"}));
    f.pin("goal-match-and-loop").await;
});

scene!(watching_and_actions, async {
    let f = Fixture::new(Control::default()).await;
    f.connect();
    f.emit(json!({"type":"watching","on":true}));
    f.emit(json!({"type":"watching","on":false}));
    f.emit(json!({"type":"action","title":"Playing from bar 17","playing":true}));
    f.emit(json!({"type":"remembered","scope":"set","note":{"id":"s1","text":"The Reese is the main bass","at":1},"pending":true}));
    f.emit(json!({"type":"remembered","scope":"producer","note":{"id":"p1","text":"Prefers short reverbs","at":1}}));
    f.emit(json!({"type":"recipe","action":"saved","name":"Drum bus","steps":3}));
    f.emit(json!({"type":"technique","action":"kept","technique":{"id":"t1","name":"Neuro from a Reese","fits":"gritty neuro basses","source":"Au5 · Neuro bass"}}));
    f.pin("watching-and-actions").await;
});
