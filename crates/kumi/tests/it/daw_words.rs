//! What a REAPER or Renoise producer sees from Kumi's app never speaks Live's words: every TUI state a session reaches
//! (connecting, idle, the DAW gone, no model, FOCUS of each view, the command menu, /help, /status, the panels, the
//! goodbye), the plain terminal's lines, and the commands' messages (bridge, doctor, report, update, uninstall). Live's
//! own screens keep their bytes: the pins say so.
#[path = "../support/live_words.rs"]
mod live_words;
#[path = "../support/models.rs"]
mod models;
#[path = "../support/tui_app.rs"]
mod support;
use async_trait::async_trait;
use kumi::daw::work_with;
use kumi::tui::app::ConnectLive;
use kumi_agent::core::contracts::*;
use kumi_common::abort::Signal;
use kumi_daw::{provider::Installed, setup::DawSetup};
use kumi_types::daw::DawId;
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

/// A session's states as the TUI draws them for `daw`, with its focus events and bodies.
async fn tui_states(daw: DawId, label: &str, focuses: Vec<(serde_json::Value, Option<FocusBody>)>) -> Found {
    work_with(daw);
    let found = Found::default();
    let c = Rc::new(Control::default());
    // Two requests to take back, the older one's change from an earlier session; a line of taste in /memory.
    let change = |id: &str, title: &str, state: &str| json!({"id":id,"family":"mixer","title":title,"state":state,"at":1});
    c.set(
        "take-back",
        json!([
            {"at":3,"words":"and a bit darker","changes":[change("c2", "Bass volume to -6 dB", "applied")]},
            {"at":2,"words":"add a hi-hat groove","changes":[change("old:c1", "Hi-hat volume to -3 dB", "expired")]}
        ]),
    );
    c.set("taste", json!([{"id":"tempo","line":"Tempo: usually 124–126 BPM"}]));
    // What keys.json gives that Kumi can't use, said as it starts.
    let keys = json!({ "composer.undoo": ["ctrl+z"], "app.redraw": ["ctrl+cmd+l"] });
    let h = Harness::with(120, 40, c.clone(), |o| o.keys = Some(Ok(keys)));
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
        // The help scrolls: each page of it, to its end.
        if command == "/help" {
            for _ in 0..12 {
                h.type_text("\x1b[6~").await;
                settle(&h).await;
                found.screen(command, &h);
            }
        }
        // A list with tabs: the other one too.
        h.type_text("\t").await;
        settle(&h).await;
        found.screen(command, &h);
        h.type_text("\x1b").await;
        settle(&h).await;
    }
    // A line of taste to forget, as /memory asks.
    h.type_text("/memory\r").await;
    h.type_text("tempo\r").await;
    settle(&h).await;
    found.screen("forgetting taste", &h);
    h.type_text("\x1b").await;
    settle(&h).await;
    // /refresh, while it reads the project.
    c.set("refresh-wait", json!(400));
    h.type_text("/refresh\r").await;
    delay(60).await;
    found.screen("/refresh", &h);
    delay(500).await;
    settle(&h).await;
    // The requests to take back, then what's asked before one is (the newest offers a copy; the older one names what
    // stays as it is), then what taking it back says.
    h.type_text("\x1b").await;
    h.type_text("\x1b").await;
    settle(&h).await;
    found.screen("take-back list", &h);
    h.type_text("\r").await;
    settle(&h).await;
    found.screen("take-back question", &h);
    h.type_text("\x1b").await;
    settle(&h).await;
    h.type_text("\x1b").await;
    h.type_text("\x1b").await;
    settle(&h).await;
    h.type_text("\x1b[B\r").await;
    settle(&h).await;
    found.screen("take-back question", &h);
    c.set("taken-back", json!({"requests":2,"words":"add a hi-hat groove","undone":[change("c2", "Bass volume to -6 dB", "undone")]}));
    h.type_text("\r").await;
    settle(&h).await;
    found.screen("taken back", &h);
    h.type_text("\x15").await;
    // The conversation in detail (ctrl+o), as the session exports it in the DAW's words.
    let words = kumi::daw::words();
    let said = |role: &str, text: &str| json!({"role": role, "content": [{"type": "text", "text": text}]});
    *c.export.borrow_mut() = Some(ConversationToExport {
        id: "now001".into(),
        title: "add a hi-hat groove".into(),
        set: Some("Night Drive".into()),
        created: now as i64 - 60_000,
        conversation: SavedConversation {
            saved_at: now as i64,
            checkpoint: KernelCheckpoint {
                version: 1,
                messages: vec![said("user", "add a hi-hat groove"), said("assistant", "The hi-hats swing now.")],
                origin: None,
                tools: None,
            },
            changes: None,
            first: Some("add a hi-hat groove".into()),
            turns: Some(1),
        },
        words: (words.app, words.place),
    });
    h.type_text("\x0f").await;
    settle(&h).await;
    found.screen("details", &h);
    h.type_text("q").await;
    settle(&h).await;
    // A window beside the DAW, under 100 columns: the drawer, folded and open.
    h.resize(80, 30);
    settle(&h).await;
    found.screen("drawer", &h);
    h.type_text("\x1c").await;
    settle(&h).await;
    found.screen("drawer", &h);
    h.type_text("\x1c").await;
    h.resize(120, 40);
    settle(&h).await;
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
        "Kumi closed. Each project's conversation continues next time.",
        // (/status: REAPER has no status bar Kumi can write to)
        "no status bar",
        "Forget this, from your Live Sets?",
        "reading your project",
        "Take a request back",
        "Carry on from here in a copy",
        "Stays as it is: Hi-hat volume to -3 dB",
        "Took back your last 2 requests",
        "The hi-hats swing now.",
        "keys.json names",
    ];
    tui_states(DawId::Reaper, "REAPER — Night Drive", focuses).await.saw(&seen.map(String::from)).assert_none("REAPER");
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
        "Forget this, from your Live Sets?",
        "reading your song",
        "Take a request back",
        "Took back your last 2 requests",
        "keys.json names",
    ];
    tui_states(DawId::Renoise, "Renoise — Night Drive", focuses).await.saw(&seen.map(String::from)).assert_none("Renoise");
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
        let h = Harness::with(120, 30, Control::with_models(models), |o| {
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
