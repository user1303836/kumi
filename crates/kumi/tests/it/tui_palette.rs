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

/// A decaying tone, as a 16-bit mono WAV: a sound on disk to make another from.
fn hit(path: &std::path::Path, hz: f64) {
    let (rate, samples) = (48_000u32, 24_000u32);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + samples * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    for (value, width) in [(16u32, 4), (1, 2), (1, 2), (rate, 4), (rate * 2, 4), (2, 2), (16, 2)] {
        bytes.extend_from_slice(&value.to_le_bytes()[..width]);
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(samples * 2).to_le_bytes());
    for n in 0..samples {
        let t = n as f64 / rate as f64;
        bytes.extend_from_slice(&((0.7 * (std::f64::consts::TAU * hz * t).sin() * (-t * 8.0).exp() * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

case!(palette_makes_a_sound_from_its_sounds_and_r_makes_another_variation, async {
    let dir = tempfile::tempdir().unwrap();
    let (kick, bell) = (dir.path().join("Kick 1.wav"), dir.path().join("Bell 2.wav"));
    hit(&kick, 60.0);
    hit(&bell, 880.0);
    let made = dir.path().join("made");
    let control = Rc::new(Control::default());
    control.set("palette", json!(true));
    let folder = made.clone();
    let h = Harness::with(140, 40, control, move |options| options.design_dir = Some(folder));
    h.start().await;
    h.connect();
    let files = [file(kick.to_str().unwrap(), "Kick 1", "kick"), file(bell.to_str().unwrap(), "Bell 2", "fx")];
    h.emit(json!({"type": "palette", "palette": {"id": "p1", "name": "Drums", "reach": 0, "files": files}}));
    h.type_text("\x1b[Z").await;
    h.type_text("\x1b[Z").await;
    let lines = h.screen();
    for text in ["MAKE from 2 sounds", "Blend", "Resonate", "Smear", "Stack"] {
        assert!(has(&lines, text), "{text}:\n{}", lines.join("\n"));
    }
    // Down past the sounds, the kit and MAKE's line to Blend: Enter makes it, and MADE lists its recipe.
    for _ in 0..4 {
        h.type_text("\x1b[B").await;
    }
    h.type_text("\r").await;
    h.wait_for("Blend of 2").await;
    // Down to the sound made: r makes another variation beside it.
    for _ in 0..9 {
        h.type_text("\x1b[B").await;
    }
    h.type_text("r").await;
    for _ in 0..500 {
        if h.screen().iter().filter(|line| line.contains("Blend of 2")).count() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(h.screen().iter().filter(|line| line.contains("Blend of 2")).count(), 2, "{}", h.screen().join("\n"));
    assert_eq!(std::fs::read_dir(&made).unwrap().count(), 2, "each variation is a file of its own");
    h.close().await;
});
