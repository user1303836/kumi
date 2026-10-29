//! ctrl+o: the conversation in detail, as the session gives it for the full export, with search, jumps between
//! requests, the terminal's scrollback and the producer's editor.
#[path = "../support/tui_app.rs"]
mod support;
use futures::FutureExt;
use kumi_agent::core::contracts::{ConversationToExport, KernelCheckpoint, SavedConversation};
use serde_json::json;
use std::{cell::RefCell, path::PathBuf, rc::Rc};
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

const CTRL_O: &str = "\x0f";

/// Two requests and their answers, as the session gives them to export.
fn conversation() -> ConversationToExport {
    let said = |role: &str, text: &str| json!({"role": role, "content": [{"type": "text", "text": text}]});
    let messages = vec![
        said("user", "make the bass wider"),
        said("assistant", "The bass is wider, and its low end stays centred."),
        said("user", "and the pad"),
        said("assistant", "The pad sits back now."),
    ];
    ConversationToExport {
        id: "k3x9a1b2".into(),
        title: "make the bass wider".into(),
        set: Some("Night Drive".into()),
        created: 1_760_000_000_000,
        conversation: SavedConversation {
            saved_at: 1_760_000_100_000,
            checkpoint: KernelCheckpoint { version: 1, messages, origin: None, tools: None },
            changes: None,
            first: Some("make the bass wider".into()),
            turns: Some(2),
        },
        words: ("Live", "Set"),
    }
}

async fn opened(h: &Harness) {
    *h.control.export.borrow_mut() = Some(conversation());
    h.type_text(CTRL_O).await;
    h.wait_for("The pad sits back now.").await;
}

case!(ctrl_o_shows_the_conversation_in_detail_and_q_esc_or_ctrl_o_closes_it, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    for close in ["q", "\x1b", CTRL_O] {
        opened(&h).await;
        h.has("make the bass wider");
        h.has("/ search · { } requests · [ scrollback · v editor · q closes");
        h.type_text(close).await;
        delay(30).await;
        assert!(!has(&h.screen(), "The pad sits back now."), "{close:?} closes it:\n{}", h.screen().join("\n"));
    }
    h.close().await;
});

case!(a_search_counts_its_matches_and_braces_go_from_request_to_request, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    opened(&h).await;
    h.type_text("/pad").await;
    h.has("search: pad · 1 of 2");
    h.type_text("\r").await;
    h.has("n N matches · / searches again · { } requests · q closes");
    h.type_text("n").await;
    h.has("search: pad · 2 of 2");
    h.type_text("N").await;
    h.has("search: pad · 1 of 2");
    // Esc in the search drops it; the next Esc closes the view.
    h.type_text("/zzz").await;
    h.has("search: zzz · no match");
    h.type_text("\x1b").await;
    delay(30).await;
    assert!(!has(&h.screen(), "search:"));
    h.has("The pad sits back now.");
    h.close().await;
});

case!(a_bracket_prints_the_details_into_the_scrollback_until_a_key, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    opened(&h).await;
    let before = h.output.text.borrow().len();
    h.type_text("[").await;
    delay(30).await;
    let printed = h.output.text.borrow()[before..].to_string();
    let left = printed.find("\x1b[?1049l").expect("the screen given back");
    assert!(printed[left..].contains("make the bass wider") && printed.contains("— Any key goes back to Kumi —"), "{printed:?}");
    // The key that brings Kumi back is the wait's: an arrow's bytes don't reach the box or the view.
    h.type_text("\x1b[A").await;
    delay(80).await;
    let back = h.output.text.borrow()[before..].to_string();
    assert!(back.rfind("\x1b[?1049h").is_some_and(|at| at > left), "the screen taken back");
    h.has("The pad sits back now.");
    h.close().await;
});

case!(v_opens_the_full_export_in_the_editor_and_its_file_stays_until_kumi_quits, async {
    let seen = Rc::new(RefCell::new(Vec::<(PathBuf, String)>::new()));
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| {
        o.editor = Some("fake-editor".into());
        let seen = seen.clone();
        o.run_editor = Some(Rc::new(move |path: PathBuf| {
            let seen = seen.clone();
            async move {
                seen.borrow_mut().push((path.clone(), std::fs::read_to_string(&path)?));
                Ok(0)
            }
            .boxed_local()
        }));
    });
    h.start().await;
    h.connect();
    opened(&h).await;
    h.type_text("v").await;
    delay(50).await;
    let (path, text) = seen.borrow().first().cloned().expect("the editor ran");
    assert!(text.contains("## You") && text.contains("make the bass wider"), "{text}");
    assert!(path.exists(), "the file stays while Kumi runs");
    h.close().await;
    assert!(!path.exists(), "and goes when it quits");
});

case!(the_turn_under_way_shows_its_steps_under_the_saved_part, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("look at the drums\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"tool-start","id":"t1","name":"live_discover"}));
    *h.control.export.borrow_mut() = Some(conversation());
    h.type_text(CTRL_O).await;
    h.wait_for("Under way").await;
    let lines = h.screen();
    let under = lines.iter().position(|line| line.contains("Under way")).unwrap();
    assert!(lines[under + 1].contains("▸ "), "{}", lines.join("\n"));
    h.close().await;
});

case!(what_the_view_covers_cant_be_clicked_through_it, async {
    // At 80 columns the drawer sits under the conversation, NOW's undo in it; the view covers both.
    let h = Harness::new(80, 30);
    h.start().await;
    h.connect();
    h.emit(json!({"type":"change","change":{"id":"c1","family":"mixer","title":"Bass width 100% → 140%","state":"applied","at":1}}));
    let lines = h.screen();
    let row = lines.iter().position(|line| line.trim_end().ends_with("undo")).expect("NOW's undo");
    let column = lines[row][..lines[row].rfind("undo").unwrap()].chars().count();
    opened(&h).await;
    h.type_text(&format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", column + 1, row + 1, column + 1, row + 1)).await;
    delay(30).await;
    assert!(!h.calls().iter().any(|call| call.starts_with("undo")), "a click through the view: {:?}", h.calls());
    h.has("The pad sits back now.");
    h.close().await;
});
