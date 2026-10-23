//! What a REAPER, Renoise or Logic producer sees from Kumi's app never speaks Live's words: every TUI state a session reaches
//! (connecting, idle, the DAW gone, no model, FOCUS of each view, the command menu, /help, /status, the panels, the
//! goodbye), the plain terminal's lines, and the commands' messages (bridge, doctor, report, update, uninstall). Live's
//! own screens keep their bytes: the pins say so.
#[path = "support/live_words.rs"]
mod live_words;
#[path = "support/models.rs"]
mod models;
#[path = "support/tui_app.rs"]
mod support;
use async_trait::async_trait;
use kumi::daw::work_with;
use kumi::tui::app::{ConnectLive, DawSetup, Installed};
use kumi_common::abort::Signal;
use kumi_runtime::core::{contracts::*, daw::DawId};
use live_words::leaks;
use regex::Regex;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use support::*;

/// Every leak found, by where it was seen; and all that was seen, to check the DAW's own words are there.
#[derive(Default)]
struct Found(RefCell<Vec<(String, String)>>, RefCell<String>);
impl Found {
    fn scan(&self, place: &str, text: &str) {
        self.1.borrow_mut().push_str(&format!("{}\n", text.split_whitespace().collect::<Vec<_>>().join(" ")));
        for line in leaks(text) {
            let mut found = self.0.borrow_mut();
            if !found.iter().any(|(_, seen)| *seen == line) {
                found.push((place.to_string(), line));
            }
        }
    }
    fn screen(&self, place: &str, h: &Harness) {
        let screen = h.screen().join("\n");
        if std::env::var("KUMI_WORDS_DEBUG").is_ok() {
            eprintln!("===== {place}\n{screen}");
        }
        self.scan(place, &screen);
    }
    /// The DAW's words were seen: the screens were drawn, not left empty.
    fn saw(&self, words: &[String]) -> &Self {
        let seen = self.1.borrow();
        for words in words {
            assert!(seen.contains(words.as_str()), "never saw {words:?} in:\n{seen}");
        }
        self
    }
    fn assert_none(&self, daw: &str) {
        let found = self.0.borrow();
        assert!(
            found.is_empty(),
            "{daw} shows Live's words:\n{}",
            found.iter().map(|(at, line)| format!("  [{at}] {line}")).collect::<Vec<_>>().join("\n")
        );
    }
}

async fn settle(h: &Harness) {
    for _ in 0..20 {
        h.screen();
        delay(5).await;
    }
}

/// A session's states as the TUI draws them for `daw`, with its focus events and bodies, and `history`: the DAW's own
/// HISTORY rows beyond the ones every DAW gets (a change only the DAW's own undo takes back, one past Kumi's undo, an
/// undo the DAW didn't finish), each as the session tells the app of it.
async fn tui_states(
    daw: DawId,
    label: &str,
    focuses: Vec<(serde_json::Value, Option<FocusBody>)>,
    history: Vec<serde_json::Value>,
) -> Found {
    work_with(daw);
    let found = Found::default();
    let c = Rc::new(Control::default());
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    settle(&h).await;
    found.screen("connecting", &h);
    h.emit(json!({"type":"connection","state":"connected"}));
    h.emit(json!({"type":"observation","label":label,"name":"Night Drive"}));
    settle(&h).await;
    found.screen("idle", &h);
    for (focus, body) in focuses {
        *c.own.borrow_mut() = body;
        h.emit(json!({"type":"focus","focus":focus}));
        settle(&h).await;
        found.screen("FOCUS", &h);
    }
    // What the session tells the app: a change to check, a catch-up after the DAW was away, a note kept once the
    // project is saved, watching, the library, another place's conversation.
    let now = now_ms();
    h.emit(json!({"type":"change","change":{"id":"c1","family":"mixer","title":"Bass volume to -6 dB","state":"unsure","at":1}}));
    h.emit(json!({"type":"remembered","scope":"set","note":{"id":"s1","text":"The Reese is the main bass","at":1},"pending":true}));
    // A catch-up is told once the conversation has something in it.
    h.emit(json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":now as i64 - 60_000,"lines":["Muted Drums"],"more":0,"afterReconnect":true}}));
    h.emit(json!({"type":"watching","on":true}));
    h.emit(json!({"type":"library","status":{"state":"paused","sounds":10,"presets":2,"sets":3}}));
    settle(&h).await;
    found.screen("notices", &h);
    h.type_text("/status\r").await;
    settle(&h).await;
    found.screen("/status", &h);
    // A change that stands, and the producer's /undo of it: the notice, and HISTORY's rows, in the DAW's words.
    h.emit(json!({"type":"change","change":{"id":"c2","family":"tempo","title":"Tempo 120 → 124 BPM","state":"applied","at":2}}));
    settle(&h).await;
    found.screen("a change", &h);
    // The DAW's own rows: kept, past Kumi's undo, unsure after an undo the DAW didn't finish.
    for change in history {
        h.emit(json!({"type":"change","change":change}));
    }
    settle(&h).await;
    found.screen("HISTORY", &h);
    *c.undo.borrow_mut() =
        Some(serde_json::from_value(json!({"id":"c2","family":"tempo","title":"Tempo 120 → 124 BPM","state":"undone","at":2})).unwrap());
    h.type_text("/undo\r").await;
    settle(&h).await;
    found.screen("/undo", &h);
    h.emit(json!({"type":"library","status":{"state":"ready","sounds":1020,"presets":40,"sets":3,"learnedAt":1}}));
    settle(&h).await;
    found.screen("library", &h);
    *c.memory.borrow_mut() = Some(serde_json::from_value(json!({"producer":[],"set":[],"saved":false})).unwrap());
    *c.conversations.borrow_mut() = serde_json::from_value(json!([
        {"id":"now001","savedAt":now as i64,"first":"add a hi-hat groove","turns":2,"current":true,"place":"reaper-x"}
    ]))
    .unwrap();
    c.set(
        "all",
        json!([
            {"id":"now001","savedAt":now as i64,"first":"add a hi-hat groove","turns":2,"current":true,"place":"reaper-x"},
            {"id":"far002","savedAt":now as i64 - 3_600_000,"first":"sketch some chords","turns":1,"current":false,"place":"unsaved"}
        ]),
    );
    h.type_text("/").await;
    settle(&h).await;
    found.screen("command menu", &h);
    // Each command's words show as the menu reaches it.
    for _ in 0..40 {
        h.type_text("\x1b[B").await;
        found.screen("command menu", &h);
    }
    h.type_text("\x1b").await;
    h.type_text("\x7f").await;
    for command in ["/help", "/status", "/conversations", "/memory", "/recipes", "/jobs"] {
        h.type_text(&format!("{command}\r")).await;
        settle(&h).await;
        found.screen(command, &h);
        // A list with tabs: the other one too.
        h.type_text("\t").await;
        settle(&h).await;
        found.screen(command, &h);
        h.type_text("\x1b").await;
        settle(&h).await;
    }
    h.emit(json!({"type":"connection","state":"disconnected"}));
    settle(&h).await;
    found.screen("the DAW gone", &h);
    h.emit(json!({"type":"connection","state":"connecting"}));
    settle(&h).await;
    found.screen("reconnecting", &h);
    h.close().await;
    // What's left on the terminal once Kumi has left its screen.
    let written = h.output.text.borrow().clone();
    let plain = Regex::new(r"\x1b\[[0-9;?<>]*[ -/]*[@-~]").unwrap().replace_all(&written, "").to_string();
    let goodbye = plain.rfind("Kumi closed").map(|at| &plain[at..]).expect("the goodbye");
    found.scan("goodbye", goodbye);
    found
}

macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

case!(a_reaper_session_never_speaks_lives_words, async {
    let track = json!({"name":"Bass","color":"#3c7cf5","kind":"midi"});
    let focuses = vec![
        (json!({"track":track,"trackRef":"r:track:{B}","mainView":{"kind":"timeline","label":"Arrange"}}), None),
        (
            json!({"track":track,"trackRef":"r:track:{B}","mainView":{"kind":"timeline","label":"Arrange"},"editorView":{"kind":"routing","label":"Routing"}}),
            Some(FocusBody::Routing(RoutingView {
                track_ref: "r:track:{B}".into(),
                name: None,
                routes: vec![],
                parent_send: Some(true),
                midi_out: Some("Launchpad · channel 2".into()),
            })),
        ),
        (
            json!({"track":track,"trackRef":"r:track:{B}","device":"ReaComp","mainView":{"kind":"timeline","label":"Arrange"},"editorView":{"kind":"device-chain","label":"FX"}}),
            None,
        ),
    ];
    let seen = [
        "connecting to REAPER",
        "Ask Kumi about your project",
        "check REAPER",
        "While REAPER was away",
        "Watching your changes in REAPER",
        "Kept once the project is saved",
        "A project not saved yet",
        "Read your REAPER project again",
        "REAPER connected",
        "REAPER isn't connected",
        "Undid: Tempo 120 → 124 BPM",
        "Ran action",
        "kept",
        "Kumi closed. Each project's conversation continues next time.",
    ];
    let history = vec![
        json!({"id":"c3","family":"structure","title":"Ran action “Track: Insert new track”","state":"kept","note":"Only REAPER's own undo can take this back (Edit > Undo).","at":3}),
    ];
    tui_states(DawId::Reaper, "REAPER — Night Drive", focuses, history).await.saw(&seen.map(String::from)).assert_none("REAPER");
});

case!(a_logic_session_never_speaks_lives_words, async {
    let track = json!({"name":"Keys","color":"#5b8def","kind":"midi"});
    let focuses = vec![
        (json!({"track":track,"trackRef":"1:track:2","mainView":{"kind":"timeline","label":"Tracks"}}), None),
        (
            json!({"track":track,"trackRef":"1:track:2","mainView":{"kind":"timeline","label":"Tracks"},"editorView":{"kind":"note-editor","label":"Piano Roll"},"item":{"kind":"item","itemRef":"1:region:1","name":"Keys"}}),
            None,
        ),
        // A dialog up in Logic: said in Logic's words; then free again, with the routing shown.
        (
            json!({"track":track,"trackRef":"1:track:2","device":"Alchemy","mainView":{"kind":"timeline","label":"Tracks"},"editorView":{"kind":"device-chain","label":"Plug-in"},"busy":"Logic Pro is showing a dialog"}),
            None,
        ),
        (
            json!({"track":track,"trackRef":"1:track:2","mainView":{"kind":"timeline","label":"Tracks"},"editorView":{"kind":"routing","label":"Routing"}}),
            Some(FocusBody::Routing(RoutingView {
                track_ref: "1:track:2".into(),
                name: None,
                routes: vec![],
                parent_send: None,
                midi_out: None,
            })),
        ),
    ];
    // HISTORY over Logic's profile: a change to check, the catch-up, watching, and the producer's /undo, each in
    // Logic's words.
    let seen = [
        "connecting to Logic Pro",
        "Ask Kumi about your project",
        "check Logic Pro",
        "While Logic Pro was away",
        "Watching your changes in Logic Pro",
        "Kept once the project is saved",
        "A project not saved yet",
        "Read your Logic Pro project again",
        "Logic Pro connected",
        "Logic Pro isn't connected",
        "Logic Pro is busy",
        "Undid: Tempo 120 → 124 BPM",
        "Ran key command",
        "kept",
        "Split region",
        "no undo",
        "Imported 64 notes",
        "Cycle bar 5",
        "Kumi closed. Each project's conversation continues next time.",
        // (/status: REAPER has no status bar Kumi can write to)
        "no status bar",
    ];
    // (M4, M4b) HISTORY's rows for Logic's two M4 slices: a key command only Logic's own undo takes back (kept, in Logic's
    // words), a split past Kumi's undo (no undo), an import whose undo Logic didn't finish (unsure, Logic's words
    // leading), and the cycle, which Logic's own undo never takes back.
    let history = vec![
        json!({"id":"c3","family":"structure","title":"Ran key command “Create Marker”","state":"kept","note":"Kumi can't take back what a key command did: Logic Pro's own undo can take it back (Cmd-Z in Logic Pro)","at":3}),
        json!({"id":"c4","family":"clip","title":"Split region “Keys” at bar 3","state":"expired","note":"Kumi no longer holds this change for its undo; Logic Pro's own undo (Cmd-Z in Logic Pro) may.","at":4}),
        json!({"id":"c5","family":"clip","title":"Imported 64 notes on “Keys” at bar 9","state":"unsure","note":"Kumi's undo didn't finish: Logic Pro didn't answer after the change was sent: the helper didn't answer in time","at":5}),
        json!({"id":"c6","family":"locators","title":"Cycle bar 5 to bar 9","state":"applied","note":"Logic Pro's own undo (Cmd-Z in Logic Pro) can't take this back; Kumi can.","at":6}),
    ];
    tui_states(DawId::Logic, "Logic Pro — Night Drive", focuses.clone(), history)
        .await
        .saw(&seen.map(String::from))
        .assert_none("Logic Pro");
    // (M4b) HISTORY's rows for the second slice, in the bridge's titles: a Gain on the Stereo Out and its gain set (no
    // step of Logic's undo), a send to a new bus undone, a patch only Logic's own undo takes back, kept, and Volume
    // automation written.
    // (the panel cuts a title at its width)
    let seen = [
        "Added “Gain” to “Stereo Out”",
        "“Gain”'s Gain → -3.0 dB",
        "Added send from “Vocal” to",
        "Loaded the patch “Steinway",
        "Wrote 3 Volume automation",
    ];
    let history = vec![
        json!({"id":"c7","family":"device","title":"Added “Gain” to “Stereo Out”","state":"applied","at":7}),
        json!({"id":"c8","family":"parameter","title":"“Gain”'s Gain → -3.0 dB on “Stereo Out”","state":"applied","note":"Logic Pro's own undo (Cmd-Z in Logic Pro) can't take this back; Kumi can.","at":8}),
        json!({"id":"c9","family":"mixer","title":"Added send from “Vocal” to Bus 1","state":"undone","at":9}),
        json!({"id":"c10","family":"device","title":"Loaded the patch “Steinway Grand Piano” on “Keys”","state":"kept","note":"Logic Pro's undo has other steps on top of the patch load, so Kumi can't put the strip back (a patch replaces the whole strip): Cmd-Z in Logic Pro can, step by step","at":10}),
        json!({"id":"c11","family":"parameter","title":"Wrote 3 Volume automation events on “Vocal”","state":"applied","at":11}),
    ];
    tui_states(DawId::Logic, "Logic Pro — Night Drive", focuses, history).await.saw(&seen.map(String::from)).assert_none("Logic Pro");
});

case!(a_renoise_session_never_speaks_lives_words, async {
    let track = json!({"name":"Bass","kind":"sequencer"});
    let grid = TrackerGrid {
        item_ref: "3:pattern:2".into(),
        name: "Drop".into(),
        lines: 64,
        from: 0,
        cursor: Some(0),
        playing: None,
        lpb: Some(4.),
        heading: None,
        facts: vec![],
        tracks: vec![TrackerTrack { name: "Bass".into(), color: None, cells: vec!["C-4 01 .. .. ..".into()] }],
    };
    let focuses = vec![
        (
            json!({"track":track,"trackRef":"3:track:2","mainView":{"kind":"pattern-editor","label":"Pattern Editor"},"item":{"kind":"pattern-track","itemRef":"3:pattern:2","name":"Drop"}}),
            Some(FocusBody::Tracker(grid)),
        ),
        (json!({"track":track,"trackRef":"3:track:2","mainView":{"kind":"mixer","label":"Mixer"}}), None),
        (
            json!({"track":track,"trackRef":"3:track:2","mainView":{"kind":"instrument","label":"Instrument"},"item":{"kind":"sample","itemRef":"3:instrument:1","name":"Kit"}}),
            Some(FocusBody::Keyzones(KeyzoneMap { item_ref: "3:instrument:1".into(), name: "Kit".into(), ..Default::default() })),
        ),
    ];
    // (/status: Renoise's status bar shows Kumi's lines)
    let seen = [
        "Ask Kumi about your song",
        "paused while Renoise plays",
        "Kumi closed. Each song's conversation continues next time.",
        "status bar too",
    ];
    tui_states(DawId::Renoise, "Renoise — Night Drive", focuses, vec![]).await.saw(&seen.map(String::from)).assert_none("Renoise");
});

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64
}

/// A DAW's side of setup as Kumi's own bridges have it: Kumi can't ask the DAW to quit, the producer does.
struct FakeOwn {
    open: RefCell<Option<String>>,
    installed: RefCell<Installed>,
    fails: Cell<bool>,
}
#[async_trait(?Send)]
impl DawSetup for FakeOwn {
    async fn open_live(&self) -> Option<String> {
        self.open.borrow().clone()
    }
    async fn ask_to_quit(&self) -> bool {
        false
    }
    async fn closed(&self, stop: &Signal) -> bool {
        loop {
            if self.open.borrow().is_none() {
                return true;
            }
            if stop.is_cancelled() {
                return false;
            }
            delay(10).await;
        }
    }
    async fn install(&self) -> Result<String, String> {
        if self.fails.get() {
            Err("The folder for Kumi's bridge can't be written.".into())
        } else {
            Ok("1.0.0".into())
        }
    }
    async fn start(&self, _: Option<String>, _: Option<String>) -> bool {
        false
    }
    async fn connect(&self) {}
    async fn version(&self, _: Option<String>) -> Option<String> {
        Some("7.82".into())
    }
    async fn installed(&self) -> Installed {
        self.installed.borrow().clone()
    }
    fn can_ask_to_quit(&self) -> bool {
        false
    }
}

/// Setup's screens for `daw`: the DAW open (restart needed), quit by the producer, the bridge failing then going in;
/// missing; never opened.
async fn setup_states(daw: DawId) -> Found {
    work_with(daw);
    let found = Found::default();
    for (open, installed, fails) in [
        (Some("/Applications/DAW.app"), Installed::Ready, true),
        (None, Installed::Missing, false),
        (None, Installed::Older("5.1".into()), false),
        (None, Installed::NeverOpened("/Applications/DAW.app".into()), false),
    ] {
        let fake =
            Rc::new(FakeOwn { open: RefCell::new(open.map(str::to_string)), installed: RefCell::new(installed), fails: Cell::new(fails) });
        // Signed in already: setup is the DAW's step alone.
        let models = models::FakeModels::catalog();
        let h = Harness::with(120, 30, Rc::new(Control::default()), |o| {
            o.models = Some(models);
            o.connect_live =
                Some(ConnectLive { why: Some("Kumi's bridge isn't in yet.".into()), bridge: "1.0.0".into(), live: fake.clone() });
        });
        h.start().await;
        settle(&h).await;
        found.screen("setup: the DAW", &h);
        h.type_text("\r").await;
        settle(&h).await;
        found.screen("setup: the DAW", &h);
        // The producer quits it: the bridge goes in (and fails once).
        *fake.open.borrow_mut() = None;
        settle(&h).await;
        found.screen("setup: installing", &h);
        fake.fails.set(false);
        h.type_text("\r").await;
        settle(&h).await;
        found.screen("setup: done", &h);
        h.type_text("\x1b").await;
        settle(&h).await;
        found.screen("setup: later", &h);
        h.close().await;
    }
    found
}

case!(reapers_and_renoises_setup_never_speak_lives_words, async {
    let reaper = [
        "REAPER is open · restart needed",
        "Kumi waits, then opens REAPER again",
        "REAPER 7 not found",
        "reaper.fm",
        "REAPER 7 hasn't been opened here yet",
    ];
    setup_states(DawId::Reaper).await.saw(&reaper.map(String::from)).assert_none("REAPER's setup");
    let renoise = ["Renoise is open · restart needed", "Renoise 3.5 not found", "renoise.com", "chat without Renoise for now"];
    setup_states(DawId::Renoise).await.saw(&renoise.map(String::from)).assert_none("Renoise's setup");
});

/// Logic's side of setup as Kumi's own bridge has it: the bridge goes into Kumi's own folder while Logic stays open
/// (nothing quits or restarts), and Accessibility, granted in System Settings, is what the bridge's status reports.
struct FakeLogic {
    open: RefCell<Option<String>>,
    installed: RefCell<Installed>,
    fails: Cell<bool>,
    granted: Cell<bool>,
}
#[async_trait(?Send)]
impl DawSetup for FakeLogic {
    async fn open_live(&self) -> Option<String> {
        self.open.borrow().clone()
    }
    async fn ask_to_quit(&self) -> bool {
        false
    }
    async fn closed(&self, _stop: &Signal) -> bool {
        false
    }
    async fn install(&self) -> Result<String, String> {
        // Long enough for the installing screen to show.
        delay(400).await;
        if self.fails.replace(false) {
            Err("The folder for Kumi's bridge can't be written.".into())
        } else {
            Ok("0.1.0".into())
        }
    }
    async fn start(&self, _: Option<String>, _: Option<String>) -> bool {
        false
    }
    async fn connect(&self) {}
    async fn version(&self, _: Option<String>) -> Option<String> {
        Some("12.3.1".into())
    }
    async fn installed(&self) -> Installed {
        self.installed.borrow().clone()
    }
    fn can_ask_to_quit(&self) -> bool {
        false
    }
    fn needs_restart(&self) -> bool {
        false
    }
    async fn enabled(&self) -> Option<bool> {
        Some(self.granted.get())
    }
    fn settings_menu(&self) -> Option<String> {
        Some("Accessibility is under System Settings > Privacy & Security: turn on the app Kumi runs in.".into())
    }
    fn surface_wait(&self) -> std::time::Duration {
        std::time::Duration::from_millis(100)
    }
}

/// Waits (a while) until the setup shows `text`, wrapped over lines or not.
async fn wait(h: &Harness, text: &str) {
    let shown = |h: &Harness| h.screen().join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    for _ in 0..50 {
        if shown(h).contains(text) {
            return;
        }
        settle(h).await;
    }
    panic!("never saw {text:?}:\n{}", h.screen().join("\n"));
}

/// Setup's screens for Logic: open (the bridge goes in at once, failing once), then Accessibility waited for, said
/// where it is, and granted; and Logic missing or older than 12.
async fn logic_setup_states() -> Found {
    work_with(DawId::Logic);
    let found = Found::default();
    let harness = |fake: &Rc<FakeLogic>| {
        let fake = fake.clone();
        let models = models::FakeModels::catalog();
        Harness::with(120, 30, Rc::new(Control::default()), move |o| {
            o.models = Some(models);
            o.connect_live = Some(ConnectLive {
                why: Some("Kumi's Logic Pro bridge isn't set up yet, so Kumi can't see your project.".into()),
                bridge: "0.1.0".into(),
                live: fake,
            });
        })
    };
    let fake = Rc::new(FakeLogic {
        open: RefCell::new(Some("/Applications/Logic Pro.app".into())),
        installed: RefCell::new(Installed::Ready),
        fails: Cell::new(true),
        granted: Cell::new(false),
    });
    let h = harness(&fake);
    h.start().await;
    wait(&h, "The bridge goes into Kumi's own folder; Logic Pro stays open.").await;
    found.screen("setup: installing", &h);
    wait(&h, "Try again").await;
    found.screen("setup: install failed", &h);
    h.type_text("\r").await;
    wait(&h, "waiting for Accessibility").await;
    found.screen("setup: Accessibility", &h);
    h.connect();
    wait(&h, "System Settings > Privacy & Security: turn on").await;
    found.screen("setup: where Accessibility is", &h);
    fake.granted.set(true);
    wait(&h, "You're set.").await;
    found.screen("setup: done", &h);
    h.close().await;
    for installed in [Installed::Missing, Installed::Older("11.2".into())] {
        let fake = Rc::new(FakeLogic {
            open: RefCell::new(None),
            installed: RefCell::new(installed),
            fails: Cell::new(false),
            granted: Cell::new(false),
        });
        let h = harness(&fake);
        h.start().await;
        wait(&h, "Look again").await;
        found.screen("setup: no Logic Pro 12", &h);
        h.type_text("\x1b").await;
        settle(&h).await;
        found.screen("setup: later", &h);
        h.close().await;
    }
    found
}

case!(logics_setup_never_speaks_lives_words_and_never_restarts_logic, async {
    let logic = [
        "The bridge goes into Kumi's own folder; Logic Pro stays open.",
        "waiting for Accessibility",
        "Allow Kumi's helper under System Settings > Privacy & Security > Accessibility",
        "Accessibility is under System Settings > Privacy & Security: turn on the app Kumi runs in.",
        "Logic Pro 12.3.1 · connected",
        "Logic Pro 12 not found",
        "apple.com/logic-pro",
        "this computer has Logic Pro 11.2",
    ];
    let found = logic_setup_states().await;
    found.saw(&logic.map(String::from)).assert_none("Logic Pro's setup");
    let seen = found.1.borrow().to_lowercase();
    for word in ["restart", "quit"] {
        assert!(!seen.contains(word), "Logic's setup says {word:?}:\n{seen}");
    }
});

/// REAPER's and Renoise's FOCUS bodies and change pictures (tui_own_bodies' goldens: each body, each family's picture in
/// NOW and HISTORY, wide, narrow and plain) speak their DAW's words, never Live's.
#[test]
fn reapers_and_renoises_bodies_and_pictures_never_speak_lives_words() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/tui/daw");
    let mut screens = 0;
    for entry in std::fs::read_dir(&folder).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let found = leaks(&text);
        assert!(found.is_empty(), "{}: {found:?}", path.display());
        screens += 1;
    }
    assert!(screens >= 50, "every scene's screens are read: {screens}");
}

// Review K11: /reconnect in REAPER or Renoise leaves each row to the integration, which retires it in the DAW's words;
// Live's note ("Kumi reconnected to Live since…; Live's own undo still can") is for Live's rows alone.
case!(a_reconnect_in_renoise_never_gives_rows_lives_note, async {
    work_with(DawId::Renoise);
    let c = Rc::new(Control::default());
    c.set("reconnect", json!(true));
    let h = Harness::with(120, 36, c, |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"change","change":{"id":"c5","family":"mixer","title":"Track 1 muted","state":"applied","at":1}}));
    h.type_text("/reconnect\r").await;
    assert!(h.calls().contains(&"reconnect".into()));
    assert!(!h.screen().iter().any(|s| s.contains("Track 1 muted") && s.contains("no undo")), "{:?}", h.screen());
    // The integration's own word for it arrives as a change, and that's what the row says.
    let note = "Kumi's link to Renoise started again, so Kumi can't undo this anymore, and Renoise's own undo doesn't take this back: neither can now, so change it by hand if needed.";
    h.emit(json!({"type":"change","change":{"id":"c5","family":"mixer","title":"Track 1 muted","state":"expired","note":note,"at":1}}));
    assert!(h.screen().iter().any(|s| s.contains("Track 1 muted") && s.contains("no undo")), "{:?}", h.screen());
    h.close().await;
    work_with(DawId::Ableton);
});
