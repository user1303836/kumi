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

case!(takeback_names_the_changes_it_leaves_as_they_are, async {
    let h = Harness::new(120, 36);
    // The last request's change is from an earlier session: Kumi can't undo it, and leaves it.
    let width = json!({"id":"old001:c4","family":"mixer","title":"Bass width 100% → 140%","state":"expired","at":1});
    h.control.set("take-back", json!([{"at":3,"words":"and a bit wider","changes":[width]}]));
    h.control.set("taken-back", json!({"requests":1,"words":"and a bit wider","undone":[]}));
    h.start().await;
    h.connect();
    h.type_text("/takeback\r").await;
    h.wait_for_call("take-back:3").await;
    h.has("Took back your last request.");
    h.has("Left as it is: Bass width");
    h.has("earlier session).");
    h.close().await;
});

/// The screen from the list titled `title` down: its rows, then the box.
fn list(h: &Harness, title: &str) -> Vec<String> {
    let shown = h.screen();
    let at = shown.iter().position(|line| line.contains(title)).unwrap_or_else(|| panic!("no {title:?}: {shown:#?}"));
    shown[at..].to_vec()
}

/// Two escs in a row, as quick as a producer's.
async fn esc_esc(h: &Harness) {
    h.type_text("\x1b").await;
    h.type_text("\x1b").await;
}

case!(esc_esc_lists_the_requests_newest_first_and_takes_one_back_once_its_confirmed, async {
    let h = two_requests().await;
    h.control.set(
        "taken-back-event",
        json!({"type":"taken-back","requests":1,"replay":{"before":0,"lines":asked(2, "make the bass wider", "Widened it.")}}),
    );
    h.control.set("taken-back", json!({"requests":1,"words":"and a bit darker","undone":[filter("undone")]}));
    esc_esc(&h).await;
    h.wait_for("Take a request back").await;
    // Newest first, each with how many changes it made.
    let shown = list(&h, "Take a request back");
    let row = |text: &str| shown.iter().position(|line| line.contains(text)).unwrap_or_else(|| panic!("{text:?}: {shown:#?}"));
    assert!(row("and a bit darker") < row("make the bass wider"), "{shown:#?}");
    assert!(shown[row("and a bit darker")].contains("1 change"), "{shown:#?}");
    assert!(shown[row("make the bass wider")].contains("no changes"), "{shown:#?}");
    // Chosen, it says what Kumi undoes, and nothing's undone until that's confirmed.
    h.type_text("\r").await;
    h.wait_for("Take back “and a bit darker”?").await;
    h.has("Undoes 1 change; its words come back");
    assert!(!h.calls().iter().any(|call| call.starts_with("take-back:")), "{:?}", h.calls());
    h.type_text("\r").await;
    h.wait_for_call("take-back:3").await;
    h.has("Took back your last request and undid 1 change.");
    let shown = h.screen();
    assert!(!has(&shown, "Lowered the filter."), "{shown:#?}");
    h.has("Widened it.");
    h.close().await;
});

case!(esc_esc_says_what_stays_as_it_is_before_an_earlier_request_goes_with_the_one_after_it, async {
    let h = Harness::new(120, 36);
    // The first request's change is from an earlier session: Kumi can't undo it.
    let width = json!({"id":"old001:c4","family":"mixer","title":"Bass width 100% → 140%","state":"expired","at":1});
    // The last one's other change was undone since: it isn't counted.
    let pad = json!({"id":"c6","family":"mixer","title":"Pad volume 0 dB → -3 dB","state":"undone","at":2});
    h.control.set(
        "take-back",
        json!([
            {"at":3,"words":"and a bit darker","changes":[filter("applied"), pad]},
            {"at":2,"words":"make the bass wider","changes":[width]}
        ]),
    );
    h.control.set("taken-back", json!({"requests":2,"words":"make the bass wider","undone":[filter("undone")]}));
    h.start().await;
    h.connect();
    esc_esc(&h).await;
    h.wait_for("Take a request back").await;
    let shown = list(&h, "Take a request back");
    assert!(shown.iter().any(|line| line.contains("and a bit darker") && line.contains("1 change")), "{shown:#?}");
    assert!(shown.iter().any(|line| line.contains("make the bass wider") && line.contains("1 can't be undone")), "{shown:#?}");
    // Not now: the list closes and nothing's taken back.
    h.type_text("\x1b[B\r").await;
    h.wait_for("Take back “make the bass wider” and the request after it?").await;
    h.has("Stays as it is: Bass width 100% → 140% (from an earlier session)");
    h.has("Take them back");
    h.type_text("\x1b[B\r").await;
    h.wait_until_hidden("Take them back").await;
    assert!(!h.calls().iter().any(|call| call.starts_with("take-back:")), "{:?}", h.calls());
    // Again, and taken back: both requests go, and the change Kumi couldn't undo is named.
    esc_esc(&h).await;
    h.wait_for("Take a request back").await;
    h.type_text("\x1b[B\r").await;
    h.wait_for("Take them back").await;
    h.type_text("\r").await;
    h.wait_for_call("take-back:2").await;
    h.has("Took back your last 2 requests and undid 1 change.");
    h.has("140% (from an earlier session).");
    h.has("to change and send again.");
    h.close().await;
});

case!(esc_esc_is_two_escs_in_a_row_that_do_nothing_else_from_an_empty_box, async {
    let h = two_requests().await;
    let opened = |h: &Harness| has(&h.screen(), "Take a request back");
    // With words in the box.
    h.type_text("and brighter").await;
    esc_esc(&h).await;
    assert!(!opened(&h));
    h.type_text("\x15").await;
    // With another key between them.
    h.type_text("\x1b").await;
    h.type_text("\x1b[D").await;
    h.type_text("\x1b").await;
    assert!(!opened(&h));
    // More than half a second apart.
    delay(600).await;
    h.type_text("\x1b").await;
    assert!(!opened(&h));
    // The first closing something (the help): it isn't half of the two.
    delay(600).await;
    h.type_text("?").await;
    h.wait_for("Keys and commands").await;
    h.type_text("\x1b").await;
    h.wait_until_hidden("Keys and commands").await;
    h.type_text("\x1b").await;
    assert!(!opened(&h));
    // Then one more: two in a row, and the list opens.
    h.type_text("\x1b").await;
    h.wait_for("Take a request back").await;
    assert!(h.calls().iter().any(|call| call == "take-back-plan"), "{:?}", h.calls());
    h.type_text("\x1b").await;
    h.wait_until_hidden("Take a request back").await;
    // The first putting a copy's toast away (a drag let go copies what it selected).
    let lines = h.screen();
    let y = lines.iter().position(|line| line.contains("Widened it.")).unwrap();
    let x = lines[y][..lines[y].find("Widened it.").unwrap()].chars().count();
    h.type_text(&format!("\x1b[<0;{};{}M\x1b[<32;{};{}M\x1b[<0;{};{}m", x + 1, y + 1, x + 7, y + 1, x + 7, y + 1)).await;
    h.has("Copied");
    esc_esc(&h).await;
    assert!(!opened(&h));
    h.type_text("\x1b").await;
    h.wait_for("Take a request back").await;
    h.type_text("\x1b").await;
    h.wait_until_hidden("Take a request back").await;
    // The first unpinning what was pinned.
    let pin = json!({"trackRef":"3:track:0","ref":"3:arrangement_clip:0:0","node":"clip","name":"Verse riff","trail":["Bass"],"siblings":[],"live":true,"track":"Bass"});
    h.emit(json!({"type":"pointed","pin":pin}));
    h.has("Bass › Verse riff  ×");
    esc_esc(&h).await;
    assert!(!has(&h.screen(), "Verse riff  ×"));
    assert!(!opened(&h));
    h.type_text("\x1b").await;
    h.wait_for("Take a request back").await;
    h.type_text("\x1b").await;
    h.wait_until_hidden("Take a request back").await;
    // The first stopping Kumi.
    h.emit(json!({"type":"state","state":"running"}));
    h.type_text("\x1b").await;
    h.wait_for_call("cancel").await;
    h.type_text("\x1b").await;
    assert!(!opened(&h));
    h.type_text("\x1b").await;
    h.wait_for("Take a request back").await;
    h.close().await;
});

case!(esc_esc_carries_on_in_a_copy_from_before_a_request_and_leaves_the_daw_as_it_is, async {
    let h = two_requests().await;
    // As the session does it: the screen is shown the copy as it stands.
    h.control.set(
        "forked-event",
        json!({"type":"forked","from":"mv66me0m3fa91c","replay":{"before":0,"lines":asked(2, "make the bass wider", "Widened it.")}}),
    );
    esc_esc(&h).await;
    h.wait_for("Take a request back").await;
    // The newest has one before it, so its question offers a copy.
    h.type_text("\r").await;
    h.wait_for("Take back “and a bit darker”?").await;
    h.has("Carry on from here in a copy");
    h.has("Leaves your DAW and the original alone");
    h.type_text("\x1b[B\r").await;
    h.wait_for_call("fork-before:3").await;
    assert!(!h.calls().iter().any(|call| call.starts_with("take-back:")), "{:?}", h.calls());
    let shown = h.screen();
    assert!(!has(&shown, "Lowered the filter."), "{shown:#?}");
    h.has("Widened it.");
    // Gone from what's shown, its words are in the box.
    h.has("and a bit darker");
    // The oldest has nothing before it to carry on from.
    h.type_text("\x15").await;
    esc_esc(&h).await;
    h.wait_for("Take a request back").await;
    h.type_text("\x1b[B\r").await;
    h.wait_for("Take back “make the bass wider” and the request after it?").await;
    assert!(!has(&h.screen(), "Carry on from here in a copy"));
    h.close().await;
});
