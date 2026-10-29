//! The conversation's view: at the end it follows what arrives; scrolled up, an entry holds it, its top row keeps the
//! request whose answer you're reading, and a chip at its foot says what came in below.
#[path = "../support/tui_app.rs"]
mod support;
use serde_json::json;
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

const WHEEL_UP: &str = "\x1b[<64;10;10M";
const PAGE_UP: &str = "\x1b[5~";
/// The conversation's header row while scrolled up, and its first row of content under it (at 120 columns and more).
const HEADER: usize = 2;
const FIRST: usize = 3;

/// A request and its answer of `lines` numbered lines.
async fn answered(h: &Harness, request: &str, lines: usize) {
    h.type_text(&format!("{request}\r")).await;
    h.emit(json!({"type":"state","state":"running"}));
    let answer: String = (1..=lines).map(|line| format!("{request}: line {line}\n")).collect();
    h.emit(json!({"type":"text","text":answer}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
    h.emit(json!({"type":"state","state":"idle"}));
}

/// The conversation's side of a row, short of where the side pane starts at any width these tests use.
fn left(line: &str) -> String {
    line.chars().take(60).collect::<String>().trim_end().to_string()
}

case!(scrolling_up_moves_the_content_by_what_was_scrolled_across_requests, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    answered(&h, "first request", 30).await;
    answered(&h, "second request", 30).await;
    let mut before: Vec<String> = h.screen().iter().map(|line| left(line)).collect();
    let (mut pinned_second, mut pinned_first) = (false, false);
    // Three rows a turn of the wheel, the first included (the header row coming in takes one of them).
    for _ in 0..14 {
        h.type_text(WHEEL_UP).await;
        let now: Vec<String> = h.screen().iter().map(|line| left(line)).collect();
        for y in 4..24 {
            assert_eq!(now[y + 3], before[y], "row {y} moved three rows down:\n{}", now.join("\n"));
        }
        pinned_second |= now[HEADER].trim() == "second request";
        pinned_first |= now[HEADER].trim() == "first request";
        before = now;
    }
    assert!(pinned_second && pinned_first, "each request pinned while its answer was read");
    h.close().await;
});

case!(a_click_on_the_pinned_request_goes_back_to_it, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    answered(&h, "the request", 60).await;
    h.type_text(PAGE_UP).await;
    let lines = h.screen();
    assert!(lines[HEADER].contains("the request") && lines[FIRST].contains("the request: line"), "{}", lines.join("\n"));
    h.type_text(&click(&lines, HEADER, "the request")).await;
    let lines = h.screen();
    assert!(left(&lines[FIRST]).ends_with("the request") && !lines[HEADER].contains("the request"), "{}", lines.join("\n"));
    h.close().await;
});

case!(a_change_of_height_keeps_the_top_row_and_a_change_of_width_its_entry, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    answered(&h, "the request", 60).await;
    h.type_text(PAGE_UP).await;
    let top = left(&h.screen()[FIRST]);
    assert!(top.contains("the request: line"), "{top}");
    // The box growing to four lines, then a taller window.
    h.type_text("\x1b[200~one\ntwo\nthree\nfour\x1b[201~").await;
    assert_eq!(left(&h.screen()[FIRST]), top);
    h.resize(120, 44);
    delay(10).await;
    assert_eq!(left(&h.screen()[FIRST]), top);
    // Narrower, the answer's lines still fit their rows: the same line stays at the top.
    h.resize(110, 44);
    delay(10).await;
    assert_eq!(left(&h.screen()[FIRST]), top);
    h.close().await;
});

case!(ctrl_end_a_message_sent_and_new_all_follow_the_end_again, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    answered(&h, "the request", 60).await;
    for (follow, what) in [("\x1b[1;5F", "ctrl+end"), ("hello again\r", "a message sent"), ("/new\r", "/new")] {
        h.type_text(PAGE_UP).await;
        h.has("newer below · page down");
        h.type_text(follow).await;
        let lines = h.screen();
        assert!(!has(&lines, "below · page down") && !has(&lines, "new below"), "{what}:\n{}", lines.join("\n"));
    }
    h.close().await;
});

case!(what_comes_in_while_scrolled_up_is_counted_below_and_the_top_holds, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    answered(&h, "the request", 60).await;
    h.type_text(PAGE_UP).await;
    let top = left(&h.screen()[FIRST]);
    for i in 1..=3 {
        h.emit(json!({"type":"notice","message":format!("note {i} while you read")}));
    }
    let lines = h.screen();
    assert_eq!(left(&lines[FIRST]), top);
    assert!(has(&lines, "3 new below · page down · ctrl+end"), "{}", lines.join("\n"));
    h.close().await;
});
