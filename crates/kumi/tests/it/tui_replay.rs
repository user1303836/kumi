//! A conversation taken up comes back from its log as it was left: the screen it drew then, drawn again through the same
//! handling, with nothing of the moment (no alert, terminal status, panel, held message or job carrying on).
#[path = "../support/models.rs"]
mod models;
#[path = "../support/tui_app.rs"]
mod support;
use serde_json::{json, Value};
use std::rc::Rc;
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

fn request(text: &str) -> Value {
    json!({"kind":"request","at":1,"text":text})
}

fn event(event: Value) -> Value {
    json!({"kind":"event","at":2,"event":event})
}

/// Two requests as they happened: steps, a change, words, a notice and each answer's end.
fn happened() -> Vec<Value> {
    let change = json!({"id":"old001:c4","family":"mixer","title":"Bass width 100% → 140%","state":"expired",
        "note":"From an earlier session, so Kumi can't undo it now.","at":1});
    vec![
        request("make the bass wider"),
        event(json!({"type":"state","state":"running"})),
        event(json!({"type":"tool-start","id":"t1","name":"live_discover"})),
        event(json!({"type":"tool-end","id":"t1","name":"live_discover","isError":false,"elapsedMs":420})),
        event(json!({"type":"tool-start","id":"t2","name":"make_changes"})),
        event(json!({"type":"change","change":change})),
        event(json!({"type":"tool-end","id":"t2","name":"make_changes","isError":false,"elapsedMs":900})),
        event(json!({"type":"text","text":"Widened it to 140%, keeping the sub mono."})),
        event(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":2100})),
        event(json!({"type":"state","state":"idle"})),
        request("and a bit darker"),
        event(json!({"type":"state","state":"running"})),
        event(json!({"type":"text","text":"Lowered the filter to 800 Hz."})),
        event(json!({"type":"notice","message":"Kumi's tools changed; the conversation continues."})),
        event(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":1300})),
        event(json!({"type":"state","state":"idle"})),
    ]
}

/// The conversation's column of the screen, its lines with words in them: the pane at its right isn't the
/// conversation's.
fn conversation(lines: &[String]) -> Vec<String> {
    lines.iter().map(|line| line.chars().take(78).collect::<String>().trim().to_string()).filter(|line| !line.is_empty()).collect()
}

/// The app after `lines` happened in it, live: each request typed and sent, each event as the session sent it.
async fn live(lines: &[Value]) -> Harness {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    for line in lines {
        match line["kind"].as_str() {
            Some("request") => {
                let text = line["text"].as_str().unwrap();
                h.type_text(&format!("{text}\r")).await;
                h.wait_for_call(&format!("submit:{text}")).await;
            }
            _ => h.emit(line["event"].clone()),
        }
    }
    h
}

/// The session's word that a conversation is taken up, with `lines` from its log to show again.
fn resumed(lines: &[Value], extra: Value) -> Value {
    let mut resumed = json!({"type":"resumed","savedAt":kumi_common::time::now_ms() - 3 * 3_600_000,"lines":[
        {"role":"user","text":"make the bass wider"},{"role":"assistant","text":"Widened it to 140%."}
    ],"replay":{"before":0,"lines":lines}});
    for (key, value) in extra.as_object().unwrap() {
        resumed[key] = value.clone();
    }
    resumed
}

/// The app with a conversation taken up from its log, `lines` in its replay.
async fn taken_up(lines: &[Value], extra: Value) -> Harness {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.emit(resumed(lines, extra));
    h
}

case!(a_conversation_taken_up_looks_as_it_did_when_it_was_left, async {
    let was = live(&happened()).await;
    let left = conversation(&was.screen());
    was.close().await;
    let back = taken_up(&happened(), json!({})).await;
    let shown = conversation(&back.screen());
    // The same column, under the line that says it's carried on (the header above both is the same).
    let at = shown.iter().position(|line| line.starts_with("Continuing your conversation from 3 hours ago.")).expect("said");
    assert_eq!([&shown[..at], &shown[at + 1..]].concat(), left, "{shown:#?}");
    // Its HISTORY row says what Kumi can't do now.
    back.has("Bass width 100% → 140%");
    back.has("no undo");
    // Idle, as it was left: nothing at work.
    back.has("Ready");
    back.close().await;
});

case!(requests_before_the_last_shown_are_counted_and_the_words_before_the_log_come_first, async {
    let h = taken_up(
        &happened(),
        json!({"replay":{"before":12,"earlier":[{"role":"user","text":"sketch a groove"},{"role":"assistant","text":"Here's a start."}],
            "lines":happened()}}),
    )
    .await;
    let shown = conversation(&h.screen());
    let at = |text: &str| shown.iter().position(|line| line.contains(text)).unwrap_or_else(|| panic!("{text}: {shown:#?}"));
    assert!(at("12 earlier messages aren't shown.") < at("sketch a groove"));
    assert!(at("Here's a start.") < at("make the bass wider"));
    h.close().await;
});

case!(a_conversation_shown_again_tells_nothing_of_the_moment_it_happened, async {
    // With a model controller, as the app has: an error live would offer its fix.
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| {
        o.models = Some(models::FakeModels::catalog());
        o.terminal = Some(kumi::tui::alerts::TerminalProfile {
            status: true,
            notify: kumi::tui::alerts::NotifyChannel::Osc9,
            ..kumi::tui::alerts::TerminalProfile::quiet()
        });
        o.alert_gap_ms = Some(60.);
    });
    h.start().await;
    h.connect();
    // Kumi's window behind another, as when an alert would go.
    h.input.write("\x1b[O");
    delay(20).await;
    h.screen();
    let before = h.output.text.borrow().len();
    let lines = vec![
        request("pick a drum kit"),
        event(json!({"type":"state","state":"running"})),
        event(json!({"type":"text","text":"Which kit?\n\n1. 808\n2. 909"})),
        event(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":40_000})),
        event(json!({"type":"state","state":"idle"})),
        request("and a bass line"),
        event(json!({"type":"state","state":"running"})),
        event(json!({"type":"error","message":"Sign in to OpenAI again.","kind":"auth","provider":"openai"})),
        event(json!({"type":"state","state":"idle"})),
        // An answer that got too long to show: Kumi stopped it then.
        request("write it all out"),
        event(json!({"type":"state","state":"running"})),
        event(json!({"type":"text","text":"Here it is: "})),
        event(json!({"type":"text","text":"x".repeat(4 * 1024 * 1024 + 1)})),
        event(json!({"type":"text","text":"what came after the cap"})),
        event(json!({"type":"turn-complete","result":{"stopReason":"cancelled"},"elapsedMs":9_000})),
        event(json!({"type":"state","state":"idle"})),
        // A request the log ends in the middle of: Kumi stopped with it.
        request("now the hats"),
        event(json!({"type":"state","state":"running"})),
        event(json!({"type":"text","text":"Adding a"})),
    ];
    h.emit(resumed(&lines, json!({})));
    let shown = h.screen();
    // Nothing went to the terminal: no status of an answer at work, done, asking or failed, and no notification.
    let written = h.output.text.borrow()[before..].to_string();
    for said in ["state=working", "state=done", "state=blocked", "state=error", "\x1b]9;"] {
        assert!(!written.contains(said), "{said}: {written:?}");
    }
    assert!(h.notified.borrow().is_empty(), "{:?}", h.notified.borrow());
    // No pick open for the answer that ended asking, no fix offered for the error, and the one cut short ends stopped,
    // not at work.
    assert!(!shown.iter().any(|line| line.contains("1–2")), "{shown:#?}");
    assert!(!has(&shown, "Sign in now"), "{shown:#?}");
    // The answer that got too long stops where it did, and nothing live is stopped for it.
    h.has("That answer got too long to show, so Kumi stopped it.");
    assert!(!has(&shown, "what came after the cap"));
    h.has("Which kit?");
    h.has("Adding a");
    h.has("Ready");
    assert!(!h.calls().iter().any(|call| call.starts_with("submit") || call.starts_with("carry") || call == "cancel"), "{:?}", h.calls());
    // The same error now, live, offers its fix.
    h.emit(json!({"type":"error","message":"Sign in to OpenAI again.","kind":"auth","provider":"openai"}));
    h.has("Sign in now");
    h.close().await;
});

case!(answers_shown_again_were_counted_by_the_session_that_answered_them, async {
    // An API key's model, whose tokens /status counts.
    let models = models::FakeModels::catalog();
    *models.model.borrow_mut() = Some("anthropic/claude-sonnet-5-5".into());
    models.signed_in.borrow_mut().insert(kumi_ai::providers::ProviderId::parse("anthropic").unwrap());
    let h = Harness::with(240, 40, Rc::new(Control::default()), |o| o.models = Some(models));
    h.start().await;
    h.connect();
    let usage = json!({"inputTokens":9000,"outputTokens":700,"cacheReadTokens":6000,"cacheWriteTokens":0});
    let lines = vec![
        request("make the bass wider"),
        event(json!({"type":"state","state":"running"})),
        event(json!({"type":"text","text":"Widened it."})),
        event(json!({"type":"turn-complete","result":{"stopReason":"completed","usage":usage},"elapsedMs":900})),
        event(json!({"type":"state","state":"idle"})),
    ];
    h.emit(resumed(&lines, json!({})));
    h.type_text("/status\r").await;
    assert!(!has(&h.screen(), "tokens in"), "{:#?}", h.screen());
    // One answered in this session counts.
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed","usage":usage},"elapsedMs":900}));
    h.type_text("/status\r").await;
    h.has("this session: 9.0k tokens in (6.0k cached), 700 out");
    h.close().await;
});

case!(an_answer_the_log_leaves_open_before_a_later_request_ends_there_stopped, async {
    let h = taken_up(
        &[
            request("make the bass wider"),
            event(json!({"type":"state","state":"running"})),
            event(json!({"type":"tool-start","id":"t1","name":"live_discover"})),
            event(json!({"type":"text","text":"Looking at the bass first."})),
            // Kumi stopped there, without ending the answer; the next run's request follows.
            request("and a bit darker"),
            event(json!({"type":"state","state":"running"})),
            event(json!({"type":"text","text":"Lowered the filter to 800 Hz."})),
            event(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":1300})),
            event(json!({"type":"state","state":"idle"})),
        ],
        json!({}),
    )
    .await;
    h.has("Lowered the filter to 800 Hz.");
    h.has("stopped");
    // The step it cut short has no time: the log doesn't say when it would have ended.
    let shown = h.screen();
    let step = shown.iter().find(|line| line.contains("looked at your Set")).unwrap_or_else(|| panic!("{shown:#?}"));
    assert!(!step.contains("0.0s"), "{step}");
    // Nothing is at work, so nothing is drawn again while nothing happens.
    let drawn = h.output.text.borrow().len();
    delay(150).await;
    h.screen();
    assert_eq!(h.output.text.borrow().len(), drawn, "something on screen is animating");
    h.close().await;
});

case!(a_change_shown_again_undoes_where_its_record_still_can_and_opens_to_its_picture, async {
    let tempo = |id: &str, state: &str| {
        json!({"id":id,"family":"tempo","title":"Tempo 120 → 124 BPM","state":state,"at":2,"from":120.0,"to":124.0,
            "picture":{"kind":"values","values":[{"name":"Tempo","from":120.0,"to":124.0,"min":20.0,"max":999.0,"fromText":"120 BPM","toText":"124 BPM"}]}})
    };
    // Taken up again in the session that made it, the change's record still works: HISTORY offers its undo.
    let lines = vec![
        request("a bit faster"),
        event(json!({"type":"state","state":"running"})),
        event(json!({"type":"change","change":tempo("c1","applied")})),
        event(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":800})),
        event(json!({"type":"state","state":"idle"})),
    ];
    let h = taken_up(&lines, json!({})).await;
    let shown = h.screen();
    let history = shown.iter().position(|line| line.contains("HISTORY")).unwrap();
    let row = history + shown[history..].iter().position(|line| line.contains("Tempo 120 → 124 BPM")).unwrap();
    assert!(shown[row].contains("undo"), "{shown:#?}");
    let mut undone = tempo("c1", "undone");
    undone.as_object_mut().unwrap().remove("picture");
    *h.control.undo.borrow_mut() = Some(serde_json::from_value(undone).unwrap());
    h.type_text(&click(&shown, row, "undo")).await;
    assert!(h.calls().contains(&"undo:c1".into()), "{:?}", h.calls());
    h.close().await;
    // From an earlier session it can't be undone, and it still opens to what it changed.
    let h = taken_up(&[request("a bit faster"), event(json!({"type":"change","change":tempo("old001:c1","expired")}))], json!({})).await;
    h.has("no undo");
    h.type_text("\x1b[Z").await;
    h.type_text("\r").await;
    h.has("Tempo 120 BPM → 124 BPM");
    h.close().await;
});
