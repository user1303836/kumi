//! A conversation taken up comes back from its log as it was left: the screen it drew then, drawn again through the same
//! handling, with nothing of the moment (no alert, terminal status, panel, held message or job carrying on).
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
    assert!(at("12 earlier requests of this conversation aren't shown.") < at("sketch a groove"));
    assert!(at("Here's a start.") < at("make the bass wider"));
    h.close().await;
});

case!(a_conversation_shown_again_tells_nothing_of_the_moment_it_happened, async {
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| {
        o.terminal = Some(kumi::tui::alerts::TerminalProfile {
            status: true,
            progress: false,
            notify: kumi::tui::alerts::NotifyChannel::Osc9,
            tmux: false,
            app_id: None,
            images: false,
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
    // No pick open for the answer that ended asking, and the one cut short ends stopped, not at work.
    assert!(!shown.iter().any(|line| line.contains("1–2")), "{shown:#?}");
    h.has("Which kit?");
    h.has("Adding a");
    h.has("Ready");
    assert!(!h.calls().iter().any(|call| call.starts_with("submit") || call.starts_with("carry")), "{:?}", h.calls());
    h.close().await;
});
