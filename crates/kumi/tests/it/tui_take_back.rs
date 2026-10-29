//! /takeback: the producer's last request (or last few) taken back. Kumi undoes their changes and the screen shows the
//! conversation as it now stands, with the earliest one's words back in the box to change and send again; and says
//! what it couldn't undo, where it stopped.
#[path = "../support/tui_app.rs"]
mod support;
use serde_json::{json, Value};
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

fn filter(state: &str) -> Value {
    json!({"id":"c5","family":"parameter","title":"Operator · Filter Freq 1.20 kHz → 800 Hz","state":state,"at":2})
}

/// A request in the log, and its answer's words.
fn asked(at: i64, text: &str, answer: &str) -> Vec<Value> {
    let event = |event: Value| json!({"kind":"event","at":at,"event":event});
    vec![
        json!({"kind":"request","at":at,"text":text}),
        event(json!({"type":"state","state":"running"})),
        event(json!({"type":"text","text":answer})),
        event(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":800})),
        event(json!({"type":"state","state":"idle"})),
    ]
}

/// The app with a conversation of two requests on screen, which it can take back.
async fn two_requests() -> Harness {
    let h = Harness::new(120, 36);
    h.control.set(
        "take-back",
        json!([
            {"at":3,"words":"and a bit darker","changes":[filter("applied")]},
            {"at":2,"words":"make the bass wider","changes":[]}
        ]),
    );
    h.start().await;
    h.connect();
    let lines = [asked(2, "make the bass wider", "Widened it."), asked(3, "and a bit darker", "Lowered the filter.")].concat();
    h.emit(json!({"type":"resumed","savedAt":kumi_common::time::now_ms(),"lines":[],"replay":{"before":0,"lines":lines}}));
    h.has("Lowered the filter.");
    h
}

case!(takeback_takes_the_last_request_back_and_puts_its_words_back_in_the_box, async {
    let h = two_requests().await;
    // As the session does it: the screen hears of it with the conversation as it now stands, then the answer.
    h.control.set(
        "taken-back-event",
        json!({"type":"taken-back","requests":1,"replay":{"before":0,"lines":asked(2, "make the bass wider", "Widened it.")}}),
    );
    h.control.set("taken-back", json!({"requests":1,"words":"and a bit darker","undone":[filter("undone")]}));
    h.type_text("/takeback\r").await;
    h.wait_for_call("take-back:3").await;
    h.has("Took back your last request and undid 1 change.");
    h.has("box to change and send again.");
    let shown = h.screen();
    assert!(!has(&shown, "Lowered the filter."), "{shown:#?}");
    h.has("Widened it.");
    // Gone from the conversation, its words are only in the box now.
    h.has("and a bit darker");
    h.close().await;
});

case!(takeback_with_a_number_goes_back_that_many_and_says_where_an_undo_stopped_it, async {
    let h = two_requests().await;
    // The first request's change couldn't be undone: only the last request went.
    h.control.set(
        "taken-back",
        json!({"requests":1,"words":"and a bit darker","undone":[],
            "failed":{"id":"c4","family":"mixer","title":"Bass width 100% → 140%","state":"kept","at":1,
                "note":"You changed it since Kumi set it, so Kumi left it."}}),
    );
    h.type_text("/takeback 2\r").await;
    h.wait_for_call("take-back:2").await;
    h.has("Took back your last request. Kumi couldn't undo Bass width");
    h.has("stopped there.");
    h.has("You changed it since Kumi set it, so Kumi left it.");
    // Not a number: what /takeback takes (the box emptied first of the words that came back).
    h.type_text("\x15/takeback all\r").await;
    h.has("/takeback takes back your last request, and /takeback 3 your last three.");
    h.close().await;
});

case!(takeback_needs_a_kept_conversation_and_kumi_at_rest, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("/takeback\r").await;
    h.has("Kumi can't take requests back here: this conversation isn't kept.");
    assert!(!h.calls().iter().any(|call| call.starts_with("take-back")), "{:?}", h.calls());
    // Offered in the / menu only where it can be done.
    h.type_text("/take").await;
    assert!(!has(&h.screen(), "Take back your last request"));
    h.close().await;
    let h = Harness::new(120, 36);
    h.control.set("take-back", json!([]));
    h.start().await;
    h.connect();
    h.type_text("/take").await;
    h.has("Take back your last request");
    h.type_text("back\r").await;
    h.has("There's no request of yours to take back yet.");
    h.close().await;
});
