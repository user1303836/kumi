//! The session's palette in the app: /palette asks the session and says its answer; PALETTE joins the strip with the
//! session's first palette, a row a sound, x takes one out, and its last row asks Kumi to load them as a kit.
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

fn harness() -> Harness {
    let control = Rc::new(Control::default());
    control.set("palette", json!(true));
    Harness::with(140, 40, control, |_| {})
}

fn file(path: &str, name: &str, class: &str) -> Value {
    json!({"path": path, "size": 10, "mtime": 20, "name": name, "song": false, "class": class})
}

/// The strip's line: the row the tabs' names are on.
fn strip(lines: &[String]) -> usize {
    lines.iter().position(|s| s.contains("HISTORY") && s.contains("───")).unwrap()
}

case!(slash_palette_asks_the_session_and_says_its_answer, async {
    let h = harness();
    h.start().await;
    h.connect();
    h.control.set("palette-said", json!("Drums is this session's palette now: pick sounds on /map and press Enter to add them."));
    h.type_text("/palette new Drums\r").await;
    h.wait_for_call("palette:new Drums").await;
    h.wait_for("Drums is this session's palette now").await;
    h.close().await;
});

case!(palette_joins_the_strip_with_the_first_palette_x_takes_a_sound_out_and_its_last_row_loads_them, async {
    let h = harness();
    h.start().await;
    h.connect();
    assert!(!h.screen()[strip(&h.screen())].contains("PALETTE"), "no PALETTE before a palette");
    let palette = json!({
        "type": "palette",
        "palette": {"id": "p1", "name": "Drums", "reach": 0, "files": [file("/s/Kick 1.wav", "Kick 1", "kick"), file("/s/Hat 2.wav", "Hat 2", "hat")]},
    });
    h.emit(palette);
    let lines = h.screen();
    assert!(lines[strip(&lines)].contains("PALETTE 2"), "{}", lines[strip(&lines)]);
    // Into the tabs, then on to PALETTE: a row a sound, its class at the right, then loading them as a kit.
    h.type_text("\x1b[Z").await;
    h.type_text("\x1b[Z").await;
    let lines = h.screen();
    for text in ["Kick 1", "kick", "Hat 2", "Load as a kit", "Drums · strict"] {
        assert!(has(&lines, text), "{text}:\n{}", lines.join("\n"));
    }
    // x takes the sound the keyboard is on out.
    h.type_text("x").await;
    h.wait_for_call("palette-remove:/s/Kick 1.wav").await;
    // Down to the last row: Enter puts what it asks in the input box, to send.
    h.type_text("\x1b[B").await;
    h.type_text("\x1b[B").await;
    h.type_text("\r").await;
    h.wait_for("Load these sounds from my palette Drums as a kit:").await;
    h.close().await;
});
