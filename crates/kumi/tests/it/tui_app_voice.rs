#[path = "support/tui_app.rs"]
mod support;
#[path = "support/voice.rs"]
mod voice;
use kumi::{tui::voice::VoiceController, voice::VoiceChange};
use kumi_runtime::voice::{VoiceError, VoiceTrouble};
use serde_json::json;
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
const PRESS: &str = "\x1b[116;5u";
const REPEAT: &str = "\x1b[116;5:2u";
const RELEASE: &str = "\x1b[116;5:3u";
const TAP: &str = "\x1b[116;5u\x1b[116;5:3u";
fn talking(voice: Rc<voice::Control>) -> Harness {
    Harness::with(120, 36, Rc::new(Control::default()), |o| o.voice = Some(voice))
}
case!(toggle_meter_write_then_send, async {
    let v = voice::Control::new("make the bass darker");
    let h = talking(v.clone());
    h.start().await;
    h.connect();
    h.has("ctrl+t to talk");
    h.type_text(TAP).await;
    assert_eq!(*v.calls.borrow(), ["listen"]);
    for s in ["Listening…", "● 0:00", "ctrl+t to stop · enter to send · esc to cancel"] {
        h.has(s)
    }
    delay(200).await;
    h.has("▇▇");
    delay(250).await;
    h.type_text(TAP).await;
    assert_eq!(*v.calls.borrow(), ["listen", "stop", "write Night Drive"]);
    assert!(h.screen().iter().any(|s| s.contains("make the bass darker") && !s.contains("│")));
    assert!(!has(&h.screen(), "Listening…"));
    assert!(!h.calls().iter().any(|c| c.starts_with("submit")));
    h.type_text("\r").await;
    assert!(h.calls().contains(&"submit:make the bass darker".into()));
    h.close().await;
});
case!(held_protocol_and_legacy_repeats, async {
    let v = voice::Control::new("make the bass darker");
    let h = talking(v.clone());
    h.start().await;
    h.type_text(PRESS).await;
    h.type_text(&format!("{REPEAT}{REPEAT}")).await;
    h.has("let go to stop · esc to cancel");
    delay(400).await;
    h.type_text(RELEASE).await;
    assert_eq!(*v.calls.borrow(), ["listen", "stop", "write "]);
    h.has("make the bass darker");
    h.close().await;
    let v = voice::Control::new("make the bass darker");
    let h = talking(v.clone());
    h.start().await;
    h.type_text("\x14").await;
    delay(300).await;
    for _ in 0..6 {
        h.type_text("\x14").await;
    }
    h.has("let go to stop");
    assert_eq!(*v.calls.borrow(), ["listen"]);
    delay(450).await;
    assert_eq!(*v.calls.borrow(), ["listen", "stop", "write "]);
    h.has("make the bass darker");
    h.type_text("\x15\x14").await;
    h.type_text("\x14").await;
    delay(450).await;
    assert_eq!(v.calls.borrow().iter().filter(|s| s.starts_with("write")).count(), 1);
    assert!(!has(&h.screen(), "make the bass darker"));
    assert!(!has(&h.screen(), "Listening…"));
    h.close().await;
});
case!(enter_sends_escape_and_control_c_cancel_first, async {
    let v = voice::Control::new("add a reverb");
    let h = talking(v.clone());
    h.start().await;
    h.type_text(TAP).await;
    delay(450).await;
    h.type_text("\r").await;
    assert!(h.calls().contains(&"submit:add a reverb".into()));
    h.emit(json!({"type":"state","state":"running"}));
    h.type_text("\x14").await;
    h.has("Listening…");
    h.type_text("\x1b").await;
    h.wait_until_hidden("Listening…").await;
    assert!(!has(&h.screen(), "Listening…"));
    assert_eq!(v.calls.borrow().last().map(String::as_str), Some("cancel"));
    assert!(!h.calls().contains(&"cancel".into()));
    h.type_text("\x14").await;
    h.type_text("\x03").await;
    assert!(!has(&h.screen(), "Listening…"));
    assert!(!h.calls().contains(&"cancel".into()));
    h.type_text("\x1b").await;
    h.wait_for_call("cancel").await;
    assert!(h.calls().contains(&"cancel".into()));
    h.close().await;
});
case!(cursor_spacing_quiet_stop_and_auto_send, async {
    let v = voice::Control::new("darker");
    let h = talking(v.clone());
    h.start().await;
    h.type_text("make the bass").await;
    h.type_text("\x14").await;
    delay(450).await;
    v.mic.spoke.set(true);
    v.mic.quiet.set(3200.0);
    delay(150).await;
    assert_eq!(*v.calls.borrow(), ["listen", "stop", "write "]);
    h.has("make the bass darker");
    v.choose(VoiceChange { send: Some(true), ..Default::default() }).unwrap();
    v.mic.spoke.set(false);
    h.type_text("\x15").await;
    h.type_text("\x14").await;
    delay(450).await;
    v.mic.spoke.set(true);
    delay(150).await;
    assert!(h.calls().contains(&"submit:darker".into()));
    h.close().await;
});
case!(permission_quiet_and_silence_fixes, async {
    let v = voice::Control::new("");
    *v.listen_error.borrow_mut()=Some(VoiceError{trouble:VoiceTrouble::Permission,message:"macOS isn't letting Terminal use the microphone. Allow it in System Settings › Privacy & Security › Microphone, then try again.".into()}.into());
    let h = talking(v.clone());
    h.start().await;
    h.type_text("\x14").await;
    h.has("macOS isn't letting Terminal use the microphone.");
    h.has("Let Kumi hear the microphone?");
    assert!(!has(&h.screen(), "Listening…"));
    h.type_text("\r").await;
    assert!(v.calls.borrow().contains(&"privacy".into()));
    h.close().await;
    for (kind, message, panel) in [
        (
            VoiceTrouble::Quiet,
            "Kumi didn't hear you: the microphone picked up only quiet. Speak a little closer, or check its input level.",
            false,
        ),
        (VoiceTrouble::Silence, "Kumi got only silence from the microphone.", true),
    ] {
        let v = voice::Control::new("");
        *v.write_error.borrow_mut() = Some(VoiceError { trouble: kind, message: message.into() }.into());
        let h = talking(v);
        h.start().await;
        h.type_text(TAP).await;
        delay(450).await;
        h.type_text(TAP).await;
        h.has(if panel { "Kumi got only silence" } else { "Kumi didn't hear you" });
        assert_eq!(has(&h.screen(), "Fix the microphone?"), panel);
        if panel {
            h.has("Choose another microphone");
            h.type_text("\x1b[B\r").await;
            h.has("Scarlett 2i2 USB");
        }
        h.close().await;
    }
});
case!(voice_panel_send_language_and_microphone, async {
    let v = voice::Control::new("make the bass darker");
    let h = talking(v.clone());
    h.start().await;
    h.type_text("/vo").await;
    h.has("/voice");
    h.type_text("\x15/voice\r").await;
    for s in ["Talk to Kumi", "Start listening", "Send when you stop", "off", "Language", "English", "Microphone", "system default"] {
        h.has(s)
    }
    h.type_text("\x1b[B\r").await;
    assert!(v.choices().send);
    h.has("What you say is sent at once");
    h.type_text("\x1b[B\r").await;
    for s in ["The language you speak", "Japanese", "Any language"] {
        h.has(s)
    }
    h.type_text("\x1b[B\r").await;
    assert_eq!(v.choices().language, "ja");
    h.has("Japanese");
    h.type_text("\x1b[B\r").await;
    h.has("System default");
    h.has("MacBook Pro Microphone");
    h.type_text("\x1b[B\x1b[B\r").await;
    assert_eq!(v.choices().microphone.as_deref(), Some("Scarlett 2i2 USB"));
    h.type_text("\x1b[A\x1b[A\x1b[A\r").await;
    h.has("Listening…");
    assert!(v.calls.borrow().contains(&"listen".into()));
    h.type_text("\x1b").await;
    h.wait_until_hidden("Listening…").await;
    h.type_text("/help\r").await;
    h.has("ctrl+t talks instead");
    h.close().await;
});
