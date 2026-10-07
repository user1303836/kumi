#[path = "support/models.rs"]
mod models;
#[path = "support/tui_app.rs"]
mod support;
use kumi::terminal::Terminal;
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
case!(startup_restore, async {
    let h = Harness::new(120, 36);
    let done = h.app.run();
    delay(5).await;
    h.connect();
    let lines = h.screen();
    assert!(lines[0].starts_with("  Kumi  ·  Night Drive"));
    assert!(lines[0].ends_with("● Live  "));
    for s in
        ["FOCUS", "NOW", "HISTORY", "Ready", "Nothing changed yet", "Kumi can see Night Drive.", "Ask Kumi about your Set", "enter to send"]
    {
        h.has(s)
    }
    assert!(h.calls().contains(&"start".into()));
    assert!(h.input.raw.get());
    h.type_text("\x03").await;
    assert_eq!(done.await, 0);
    assert!(h.calls().contains(&"close".into()));
    assert!(!h.input.raw.get());
    assert!(h
        .output
        .text
        .borrow()
        .ends_with(&format!("{}Kumi closed. Each Set's conversation continues next time.\n", kumi::tui::tty::RESTORE)));
    h.close().await;
});
case!(typing_streaming_steps_markdown, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("what's on 主旋律?").await;
    h.has("what's on 主旋律?");
    h.type_text("\r").await;
    assert!(h.calls().contains(&"submit:what's on 主旋律?".into()));
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"The kick and bass are fighting around 200 Hz, private-token."}));
    h.emit(json!({"type":"tool-start","id":"t1","name":"live_discover"}));
    h.has("The kick and bass are fighting around 200 Hz, [redacted].");
    h.has("looking at your Set");
    h.has("working");
    h.has("esc to stop");
    assert!(!h.output.text.borrow().contains("private-token"));
    h.emit(json!({"type":"tool-end","id":"t1","name":"live_discover","isError":false,"elapsedMs":300}));
    h.has("│ ✓ looked at your Set");
    h.has("0.3s");
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":3100}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.has("▾ 1 step · 3.1s");
    h.has("Ready");
    assert!(!has(&h.screen(), "live_discover"));
    h.type_text("what's on the bass?\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"**Bass** has:\n- EQ Eight\n- Compressor with `attack 12 ms`"}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
    h.emit(json!({"type":"state","state":"idle"}));
    for s in ["Bass has:", "• EQ Eight", "• Compressor with attack 12 ms"] {
        h.has(s)
    }
    assert!(!has(&h.screen(), "**"));
    h.close().await;
});
case!(cancel_clear_and_commands, async {
    let h = Harness::new(120, 36);
    let done = h.app.run();
    delay(5).await;
    h.emit(json!({"type":"state","state":"running"}));
    h.type_text("\x1b").await;
    h.wait_for_call("cancel").await;
    assert!(h.calls().contains(&"cancel".into()));
    h.emit(json!({"type":"state","state":"idle"}));
    h.type_text("draft").await;
    h.has("draft");
    h.type_text("\x03").await;
    assert!(!has(&h.screen(), "draft"));
    assert!(!h.calls().contains(&"close".into()));
    h.type_text("/").await;
    h.has("Forget this conversation and start fresh");
    h.has("/refresh");
    assert!(!has(&h.screen(), "Read your Live Set again"));
    h.type_text("\x1b[B\x1b[B").await;
    h.has("Read your Live Set again");
    h.type_text("\r").await;
    assert!(h.calls().contains(&"refresh".into()));
    h.type_text("/q").await;
    h.has("Close Kumi");
    assert!(!has(&h.screen(), "/new"));
    h.type_text("\x1b").await;
    h.wait_until_hidden("Close Kumi").await;
    assert!(!has(&h.screen(), "Close Kumi"));
    h.type_text("\x15/nope\r").await;
    h.has("There's no /nope command. Type / to see them.");
    h.type_text("\x15/Users/me/ref.wav\r").await;
    assert!(h.calls().contains(&"submit:/Users/me/ref.wav".into()));
    h.type_text("\x03").await;
    assert_eq!(done.await, 0);
    h.close().await;
});
case!(narrow_resize_paste_and_scroll, async {
    let h = Harness::new(80, 24);
    h.start().await;
    h.connect();
    assert!(!has(&h.screen(), "HISTORY"));
    assert!(!has(&h.screen(), "FOCUS"));
    assert!(has(&h.screen()[15..], "Night Drive"));
    h.has("Ask Kumi about your Set");
    h.resize(90, 28);
    delay(10).await;
    assert_eq!(h.screen().len(), 28);
    h.has("Kumi  ·  Night Drive");
    h.type_text("\x1b[200~line one\nline two\nline three\x1b[201~").await;
    for s in ["line one", "line two", "line three"] {
        h.has(s)
    }
    assert!(!h.calls().iter().any(|c| c.starts_with("submit")));
    h.type_text("\x03").await;
    h.resize(100, 24);
    delay(10).await;
    for i in 1..=30 {
        h.emit(json!({"type":"notice","message":format!("note number {i}")}));
    }
    h.has("note number 30");
    h.type_text("\x1b[5~").await;
    let lines = h.screen();
    let top = lines.iter().find(|s| s.contains("note number")).cloned().unwrap();
    assert!(!has(&lines, "note number 30"));
    h.has("newer below · page down");
    h.emit(json!({"type":"notice","message":"note number 31"}));
    assert_eq!(h.screen().iter().find(|s| s.contains("note number")).unwrap(), &top);
    h.type_text("\x1b[6~\x1b[6~\x1b[6~").await;
    h.has("note number 31");
    h.close().await;
});
case!(dragged_files_go_with_the_next_message, async {
    let dir = tempfile::tempdir().unwrap();
    let picture = dir.path().join("Screen Shot.png");
    std::fs::write(&picture, vec![1u8; 1500]).unwrap();
    let dragged = format!("\x1b[200~'{}'\x1b[201~", picture.display());
    let h = Harness::new(120, 30);
    h.start().await;
    h.connect();
    h.type_text(&dragged).await;
    h.has("with Screen Shot.png · PNG picture · 2 KB ×");
    // The same file twice is added once, and backspace in an empty box takes the last one back.
    h.type_text(&dragged).await;
    assert_eq!(h.screen().iter().filter(|line| line.contains("Screen Shot.png")).count(), 1);
    h.type_text("\x7f").await;
    assert!(!has(&h.screen(), "Screen Shot.png"));
    h.type_text(&dragged).await;
    // A send that's refused keeps the file, to change or send again.
    h.control.set("submit-error", json!("Screen Shot.png is too big."));
    h.type_text("make this\r").await;
    h.has("Screen Shot.png is too big.");
    h.has("with Screen Shot.png");
    h.control.extra.borrow_mut().as_object_mut().unwrap().remove("submit-error");
    h.type_text("make this\r").await;
    assert!(h.calls().contains(&"submit-with:make this [Screen Shot.png image/png 1500]".into()));
    assert!(!has(&h.screen(), "with Screen Shot.png"));
    // Words that aren't a file paste as words.
    h.type_text("\x1b[200~/no/such/file.png\x1b[201~").await;
    h.has("/no/such/file.png");
    h.close().await;
});
case!(failure_disconnect_focus, async {
    for (w, rows) in [(120, 36), (80, 24)] {
        let h = Harness::new(w, rows);
        h.start().await;
        h.connect();
        h.emit(json!({"type":"focus","focus":{"track":{"name":"Bass","color":"#f59a3c","kind":"midi"},"device":"Operator","detail":"Device","view":"Session","parameter":{"name":"Filter Freq","value":"1.20 kHz","owner":"Operator"}}}));
        h.has("■ Bass › Operator › Filter Freq");
        h.has(if w == 120 { "1.20 kHz · Session · Device view" } else { "Filter Freq · 1.20 kHz" });
        h.emit(json!({"type":"connection","state":"disconnected"}));
        assert!(!has(&h.screen(), "■ Bass"));
        h.close().await;
    }
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("hello\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"error","message":"anthropic rejected the credentials (HTTP 401). Check the configured model."}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.emit(json!({"type":"connection","state":"disconnected"}));
    h.emit(json!({"type":"notice","message":"Live closed. Kumi will pick up where you left off when it's back."}));
    h.has("Kumi couldn't answer that; see the note below.");
    h.has("anthropic rejected the credentials (HTTP 401)");
    h.has("Live closed. Kumi will pick up where you left off");
    assert!(h.screen()[0].ends_with("● Live not connected  "));
    h.close().await;
});
case!(undo_history_and_clipboard, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("/undo\r").await;
    h.has("There's nothing of Kumi's to undo.");
    assert!(!h.calls().iter().any(|c| c.starts_with("undo:")));
    let tempo = json!({"id":"c1","family":"tempo","title":"Tempo 120 → 124 BPM","state":"applied","from":120,"to":124,"at":1});
    h.emit(json!({"type":"change","change":tempo}));
    h.emit(json!({"type":"change","change":{"id":"c2","family":"mixer","title":"Bass volume 0.0 dB → -2.0 dB, pan C → 5L","track":{"name":"Bass","color":"#f59a3c"},"state":"applied","from":0.85,"to":0.6,"at":2}}));
    h.has("✓ Bass volume 0.0 dB");
    let lines = h.screen();
    let history = lines.iter().position(|s| s.contains("HISTORY")).unwrap();
    assert!(lines[history + 1].contains("■ Bass volume 0.0 dB → -2.0 dB,"));
    assert!(lines[history + 1].contains("undo"));
    assert!(lines[history + 2].contains("pan C → 5L"));
    assert!(lines[history + 3].contains("✓ Tempo 120 → 124 BPM"));
    let mut undone = tempo;
    undone["state"] = json!("undone");
    *h.control.undo.borrow_mut() = Some(serde_json::from_value(undone).unwrap());
    h.type_text(&click(&lines, history + 3, "undo")).await;
    assert!(h.calls().contains(&"undo:c1".into()));
    h.has("○ Tempo 120 → 124 BPM");
    h.has("Undid: Tempo 120 → 124 BPM");
    h.type_text("/copy\r").await;
    h.has("There's no answer to copy yet.");
    h.type_text("hi\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Try a **shorter** release."}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":10}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.type_text("/copy\r").await;
    use base64::Engine;
    let expected = base64::engine::general_purpose::STANDARD.encode("Try a **shorter** release.");
    assert!(h.output.text.borrow().contains(&format!("\x1b]52;c;{expected}\x07")));
    h.has("Copied Kumi's last answer.");
    h.close().await;
});
case!(models_filter_keys_redaction, async {
    let fake = models::FakeModels::catalog();
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.models = Some(fake.clone()));
    h.start().await;
    h.connect();
    h.has("GPT-6 Astra");
    h.type_text("/model\r").await;
    for s in [
        "Choose a model",
        "type to filter",
        "signed in",
        "Frontier model for complex work",
        "Sign in to Anthropic",
        "with an API key",
        "↑↓ to move · enter to choose · esc to close",
    ] {
        h.has(s)
    }
    h.type_text("luna").await;
    h.has("filter: luna");
    h.has("GPT-6 Luna");
    assert!(!has(&h.screen()[1..], "GPT-6 Astra"));
    assert!(!has(&h.screen()[1..], "Anthropic"));
    h.type_text("\r").await;
    assert!(fake.calls.borrow().contains(&"choose:openai-codex/gpt-6-luna".into()));
    h.has("Kumi talks to GPT-6 Luna from your next message, at its usual low effort.");
    assert!(!has(&h.screen(), "Choose a model"));
    h.type_text("/login\r").await;
    h.has("Sign in to");
    h.type_text("\x1b[B\r").await;
    h.has("Paste your Anthropic API key. It stays hidden, even here.");
    let refused = "refused-key-0123456789";
    h.type_text(&format!("\x1b[200~{refused}\x1b[201~")).await;
    h.has(&"•".repeat(refused.len()));
    h.has(&format!("{} characters", refused.len()));
    h.type_text("\r").await;
    h.has("Anthropic didn't accept that key. Paste it again, or esc to leave it.");
    let key = "sk-ant-private-0123456789abcdef";
    h.type_text(&format!("\x1b[200~{key}\x1b[201~\r")).await;
    h.has("Signed in to Anthropic.");
    h.emit(json!({"type":"notice","message":format!("the provider echoed {key}")}));
    h.screen();
    for secret in [refused, key] {
        assert!(!h.output.text.borrow().contains(secret));
    }
    h.close().await;
});
case!(held_now_after_take_back_and_stop, async {
    let c = Rc::new(Control::default());
    c.steer_enabled.set(true);
    c.steer_result.set(false);
    let h = Harness::with(120, 36, c.clone(), |_| {});
    h.start().await;
    h.connect();
    h.type_text("build me a reverb\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"tool-start","id":"t1","name":"search_web"}));
    h.has("Tell Kumi more while it works");
    h.type_text("make it darker").await;
    h.has("enter sends now · tab after · esc stops");
    h.type_text("\r").await;
    assert!(h.screen().iter().any(|s| s.contains("↳ make it darker") && s.contains("at the next step")));
    c.steer_result.set(true);
    h.emit(json!({"type":"tool-end","id":"t1","name":"search_web","isError":false,"elapsedMs":400}));
    assert!(h.calls().contains(&"steer:make it darker".into()));
    h.type_text("and save it as a preset\t").await;
    assert!(h.screen().iter().any(|s| s.contains("↳ and save it as a preset") && s.contains("after this answer")));
    h.emit(json!({"type":"steer","text":"make it darker"}));
    h.emit(json!({"type":"text","text":"Darker it is."}));
    let lines = h.screen();
    assert!(!has(&lines, "↳ make it darker"));
    let at = |s: &str| lines.iter().position(|l| l.contains(s)).unwrap();
    assert!(at("searched the web") < at("make it darker") && at("make it darker") < at("Darker it is."));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":2000}));
    h.emit(json!({"type":"state","state":"idle"}));
    delay(5).await;
    assert!(h.calls().contains(&"submit:and save it as a preset".into()));
    h.emit(json!({"type":"state","state":"running"}));
    c.steer_result.set(false);
    h.type_text("first\r").await;
    h.type_text("second\t").await;
    h.type_text("\x1b[1;3A").await;
    h.has("second");
    assert!(!has(&h.screen(), "↳ second"));
    h.type_text("\x15").await;
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"cancelled"},"elapsedMs":900}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.has("first");
    assert!(!has(&h.screen(), "↳ first"));
    assert!(!h.calls().contains(&"submit:first".into()));
    h.close().await;
    let h = Harness::new(120, 36);
    h.start().await;
    h.emit(json!({"type":"state","state":"running"}));
    h.type_text("what's the tempo?\r").await;
    h.has("what's the tempo?");
    assert!(!h.calls().iter().any(|s| s.starts_with("submit:")));
    h.emit(json!({"type":"state","state":"idle"}));
    delay(10).await;
    assert!(h.calls().contains(&"submit:what's the tempo?".into()));
    h.close().await;
});
case!(welcome_catch_up_resumed_and_resend, async {
    use kumi::tui::logo::{LOGO_LETTERS, LOGO_RULE};
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    for text in LOGO_LETTERS {
        h.has(text.trim())
    }
    h.has(LOGO_RULE);
    let last = kumi_common::time::now_ms() - 3 * 86400000;
    h.emit(json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":last,"lines":["Tempo 120 → 124 BPM","Added track “Pad”"],"more":2}}));
    for s in ["Since you were last here · 3 days ago", "• Tempo 120 → 124 BPM", "• Added track “Pad”", "and 2 more changes", "Try"]
    {
        h.has(s)
    }
    h.type_text("hello\r").await;
    h.emit(json!({"type":"catch-up","catchUp":{"set":"Night Drive","lastSeenAt":last,"lines":[],"more":0}}));
    h.has("Nothing changed in Night Drive since you were last here, 3 days ago.");
    h.emit(json!({"type":"resumed","savedAt":kumi_common::time::now_ms()-2*3600000,"lines":[{"role":"user","text":"Make the pad wider"},{"role":"assistant","text":"Widened the **Pad** chorus."}]}));
    for s in ["Continuing your conversation from 2 hours ago. /new starts fresh.", "Make the pad wider", "Widened the Pad chorus."] {
        h.has(s)
    }
    h.emit(json!({"type":"resend","text":"record the chorus into a new track"}));
    h.has("record the chorus into a new track");
    h.type_text("\r").await;
    assert!(h.calls().contains(&"submit:record the chorus into a new track".into()));
    h.close().await;
    let short = Harness::new(120, 16);
    short.start().await;
    short.connect();
    assert!(!has(&short.screen(), LOGO_RULE));
    short.has("Kumi can see Night Drive.");
    short.close().await;
});
case!(planning_quiet_memory_and_watching, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("build me a pad\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.has("thinking");
    h.emit(json!({"type":"tool-input","id":"p1","name":"make_changes"}));
    h.has("writing the plan");
    h.emit(json!({"type":"tool-start","id":"p1","name":"make_changes"}));
    h.has("making changes");
    assert!(!has(&h.screen(), "writing the plan"));
    for (id, title) in [(1, "Added track Pad"), (2, "Loaded Wavetable on Pad")] {
        h.emit(json!({"type":"change","change":{"id":format!("c{id}"),"family":"structure","title":title,"state":"applied","at":id}}));
        h.has(&format!("✓ {title}"));
        h.has(&format!("working · {id} change"));
    }
    h.emit(json!({"type":"tool-end","id":"p1","name":"make_changes","isError":false,"elapsedMs":2400}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":5200}));
    h.emit(json!({"type":"state","state":"idle"}));
    assert!(!has(&h.screen(), "working ·"));
    h.close().await;
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.emit(json!({"type":"watching","on":true}));
    h.has("Watching your changes in Live");
    h.emit(json!({"type":"watching","on":false}));
    h.has("Ready");
    h.close().await;
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("the reese is my main bass\r").await;
    for e in [
        json!({"type":"state","state":"running"}),
        json!({"type":"text","text":"Got it."}),
        json!({"type":"tool-start","id":"m1","name":"remember"}),
        json!({"type":"remembered","scope":"set","note":{"id":"s1","text":"The Reese is the main bass","at":1}}),
        json!({"type":"tool-end","id":"m1","name":"remember","isError":false,"elapsedMs":3}),
        json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}),
        json!({"type":"state","state":"idle"}),
        json!({"type":"remembered","scope":"producer","note":{"id":"p1","text":"Prefers short reverbs","at":2}}),
        json!({"type":"remembered","scope":"producer","note":{"id":"p1","text":"Prefers short, dark reverbs","at":3},"replaced":{"id":"p1","text":"Prefers short reverbs","at":2}}),
        json!({"type":"forgot","scope":"set","note":{"id":"s1","text":"The Reese is the main bass","at":1}}),
    ] {
        h.emit(e)
    }
    for s in [
        "✎ Noted about Night Drive: The Reese is the main bass",
        "✎ Noted about you: Prefers short reverbs",
        "✎ Updated a note about you: Prefers short, dark reverbs",
        "✎ Forgot: The Reese is the main bass",
    ] {
        h.has(s)
    }
    assert!(!has(&h.screen(), "step"));
    h.close().await;
});
case!(heard_spectrum_comparison_above_answer, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.emit(
        json!({"type":"heard","file":"ref.wav","summary":"−8.4 LUFS · 128 BPM · F minor","bands":[-8,-5,-7,-9,-10,-12,-13,-16,-18,-22]}),
    );
    h.emit(json!({"type":"heard","file":"mix.wav","summary":"−12.1 LUFS","bands":[-8,-5,-7,-6,-10,-12,-15,-17,-19,-22],"compared":{"reference":"ref.wav","summary":"−8.4 LUFS","differences":[0.2,0.4,-0.3,2.8,0,-1.1,-2,-1.5,-0.8,0.5],"headlines":["low mids +2.8 dB"]}}));
    let lines = h.screen();
    h.has("Heard ref.wav · −8.4 LUFS · 128 BPM · F minor");
    assert!(lines.iter().any(|s| s.contains("████") && s.contains("▄▄▄▄")));
    h.has("Heard mix.wav against ref.wav, loudness matched");
    assert!(lines.iter().any(|s| s.contains("+2.8") && s.contains("−2.0")));
    assert_eq!(lines.iter().filter(|s| s.contains("l.mid") && s.contains("air")).count(), 2);
    h.close().await;
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("compare these\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Your mix is darker than the reference."}));
    h.emit(json!({"type":"heard","file":"mix.wav","summary":"−12.1 LUFS","bands":[-8,-5,-7,-6,-10,-12,-15,-17,-19,-22],"compared":{"reference":"ref.wav","summary":"−8.4 LUFS","differences":[0.2,0.4,-0.3,2.8,0,-1.1,-2,-1.5,-0.8,0.5],"headlines":[]}}));
    let lines = h.screen();
    assert!(
        lines.iter().position(|s| s.contains("Heard mix.wav")).unwrap()
            < lines.iter().position(|s| s.contains("Your mix is darker")).unwrap()
    );
    h.close().await;
});
case!(match_auditions_scores_history, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("make my pad sound like this reference\r").await;
    for e in [
        json!({"type":"state","state":"running"}),
        json!({"type":"match","state":"running","check":0,"elapsedMs":5000,"roundsLeft":12}),
        json!({"type":"auditioned","round":1,"best":{"label":"Collision","score":58},"takes":[{"label":"Collision","score":58},{"label":"Operator","score":41},{"label":"Drift","silent":true}],"gaps":["attack too slow (40 ms against 5 ms)","air (10000–20k Hz) −6.0 dB against the reference"]}),
        json!({"type":"change","change":{"id":"a1","family":"clip","title":"Auditioned 3 candidates · best Collision","state":"heard","score":58,"at":1}}),
        json!({"type":"auditioned","round":2,"best":{"label":"Collision","score":71},"previous":58,"takes":[{"label":"Collision","score":71}],"gaps":["darker overall (−1.2 dB/octave)"]}),
        json!({"type":"match","state":"running","check":2,"first":58,"best":{"label":"Collision","score":71},"elapsedMs":125000,"roundsLeft":10}),
    ] {
        h.emit(e)
    }
    for s in [
        "Round 1 · 58% · attack too slow, air (10000–20k Hz) −6.0 dB",
        "Collision 58 · Operator 41 · Drift silent",
        "Round 2 · 58% → 71% · darker overall",
        "matching · 58→71% · 2:05",
        "♪ Auditioned 3 candidates",
    ] {
        h.has(s)
    }
    assert!(h.screen().iter().any(|s| s.contains("♪ Auditioned 3 candidates · best") && s.contains("58%")));
    h.emit(json!({"type":"match","state":"done","check":3,"first":58,"best":{"label":"Collision","score":76},"elapsedMs":250000,"roundsLeft":9,"stop":"plateau"}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":250000}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.has("Matching: 58% → 76% (Collision) · 4:10 · no more gain");
    assert!(!has(&h.screen(), "matching ·"));
    h.close().await;
});
case!(watched_video_and_web_grouping_redaction, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("watch this tutorial\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"tool-start","id":"w1","name":"watch_video"}));
    h.emit(json!({"type":"doing","text":"transcribing what's said · 40%"}));
    h.has("transcribing what's said · 40%");
    h.emit(json!({"type":"text","text":"It builds a Reese bass."}));
    let frames: Vec<_> =
        [5, 15, 25, 35, 45].into_iter().map(|at| json!({"at":at,"thumb":{"width":32,"height":18,"rgb":vec![120;32*18*3]}})).collect();
    h.emit(json!({"type":"watched","title":"1 Minute Reese With Operator","channel":"Au5","url":"https://www.youtube.com/watch?v=W87uuuGcq9c","duration":81,"from":0,"to":81,"chapters":[],"words":"transcribed","lines":6,"frames":frames,"notes":["private-token leaked into a note"]}));
    h.emit(json!({"type":"tool-end","id":"w1","name":"watch_video","isError":false,"elapsedMs":1200}));
    let lines = h.screen();
    assert!(
        lines.iter().position(|s| s.contains("Watched “1 Minute Reese With Operator” · Au5 · 1:21")).unwrap()
            < lines.iter().position(|s| s.contains("It builds a Reese bass.")).unwrap()
    );
    h.has("the whole video · its speech, transcribed by Kumi");
    h.has("▀▀▀▀▀▀▀▀▀▀");
    assert!(lines.iter().any(|s| s.contains("0:05") && s.contains("0:15")));
    assert!(!has(&lines, "private-token"));
    assert!(!has(&lines, "transcribing what's said · 40%"));
    h.close().await;
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("make me a reverb like the erbe-verb\r").await;
    for e in [
        json!({"type":"state","state":"running"}),
        json!({"type":"tool-start","id":"s1","name":"search_web"}),
        json!({"type":"doing","text":"searching the web for “erbe-verb design”"}),
    ] {
        h.emit(e)
    }
    h.has("searching the web for “erbe-verb");
    for e in [
        json!({"type":"web","action":"searched","title":"erbe-verb design","where":"web","via":"Exa","results":8}),
        json!({"type":"tool-end","id":"s1","name":"search_web","isError":false,"elapsedMs":900}),
        json!({"type":"web","action":"read","title":"Building the Erbe-Verb private-token","url":"https://forum.audulus.com/uploads/erbe.pdf","kind":"a PDF","via":"Exa"}),
        json!({"type":"web","action":"read","title":"Afturmath/dm-Erbeverb","url":"https://github.com/Afturmath/dm-Erbeverb","kind":"a GitHub repository","files":29}),
        json!({"type":"text","text":"It's a four-delay FDN reverb."}),
    ] {
        h.emit(e)
    }
    let lines = h.screen();
    let at = |s: &str| lines.iter().position(|l| l.contains(s)).unwrap();
    assert_eq!(at("Read “Building the Erbe-Verb"), at("Searched the web for “erbe-verb design” · 8 results") + 1);
    assert_eq!(at("Read “Afturmath/dm-Erbeverb” · github.com · a GitHub repository · 29 files"), at("Read “Building the Erbe-Verb") + 1);
    assert!(at("It's a four-delay FDN reverb.") > at("Read “Afturmath"));
    assert!(!has(&lines, "private-token"));
    h.close().await;
});
case!(effort_logout_auth_error_resends, async {
    let fake = models::FakeModels::catalog();
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.models = Some(fake.clone()));
    h.start().await;
    h.type_text("/effort\r").await;
    h.has("How hard GPT-6 Astra thinks · lower answers sooner");
    assert!(h.screen().iter().any(|s| s.contains("Default (medium)") && s.contains("current")));
    h.has("More thorough still");
    h.type_text("\x1b[B\r").await;
    assert!(fake.calls.borrow().contains(&"effort:low".into()));
    h.has("GPT-6 Astra thinks at low effort from your next message.");
    h.has("GPT-6 Astra · low");
    h.type_text("/logout\r").await;
    h.has("Your ChatGPT sign-in");
    h.type_text("\r").await;
    h.has("Sign out of ChatGPT?");
    h.type_text("\r").await;
    assert!(fake.calls.borrow().contains(&"signout:openai-codex".into()));
    h.has("Signed out of ChatGPT.");
    h.close().await;
    let fake = models::FakeModels::catalog();
    *fake.model.borrow_mut() = Some("anthropic/claude-sonnet-5-5".into());
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.models = Some(fake.clone()));
    h.start().await;
    h.has("Sign in to Anthropic?");
    h.type_text("\x1b").await;
    h.wait_until_hidden("Sign in to Anthropic?").await;
    h.type_text("How do I tame the snare?\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"error","message":"Not signed in to Anthropic: add its API key with /login (or set ANTHROPIC_API_KEY).","kind":"auth","provider":"anthropic"}));
    h.emit(json!({"type":"state","state":"idle"}));
    for s in ["Not signed in to Anthropic", "Sign in to Anthropic?", "Sign in now", "Choose another model"] {
        h.has(s)
    }
    h.type_text("\r").await;
    h.has("Paste your Anthropic API key");
    h.type_text("\x1b[200~sk-ant-fixture-0000\x1b[201~\r").await;
    h.has("Signed in to Anthropic.");
    h.has("Sending your message again.");
    assert_eq!(h.calls().iter().filter(|c| c.starts_with("submit:")).count(), 2);
    h.close().await;
});
case!(device_tree_reads_mouse_keyboard_and_selection_colors, async {
    let c = Rc::new(Control::default());
    *c.tree.borrow_mut()=Some(serde_json::from_value(json!({"trackRef":"3:track:3","devices":[{"ref":"d0","name":"Chorus-Ensemble","className":"Chorus-Ensemble","deviceType":"audio_effect"},{"ref":"d1","name":"Compressor","className":"Compressor","deviceType":"audio_effect"},{"ref":"d2","name":"Audio Effect Rack","className":"AudioEffectGroupDevice","canHaveChains":true,"chains":[{"ref":"c0","name":"Chain 1","devices":[{"ref":"d2a","name":"Saturator","className":"Saturator","deviceType":"audio_effect"},{"ref":"d2b","name":"EQ Eight","className":"EQ Eight","deviceType":"audio_effect"}]},{"ref":"c1","name":"Chain 2","devices":[{"ref":"d2c","name":"Utility","className":"Utility","deviceType":"audio_effect"}]}]},{"ref":"d3","name":"Gate","className":"Gate","deviceType":"audio_effect"}]})).unwrap());
    let h = Harness::with(120, 40, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let focus = json!({"track":{"name":"4-Audio","color":"#e2b93b","kind":"audio"},"trackRef":"3:track:3","device":"Saturator","chain":"Chain 1","detail":"Device","view":"Session"});
    h.emit(json!({"type":"focus","focus":focus}));
    delay(5).await;
    for s in [
        "FOCUS · Device",
        "■  4-Audio",
        "├ ≈  Chorus-Ensemble",
        "├ ▣  Audio Effect Rack",
        "│ ├ ○  Chain 1",
        "│ │ ├ ≈  Saturator",
        "│ │ └ ≈  EQ Eight",
        "│ └ ○  Chain 2 (1)",
        "└ ≈  Gate",
    ] {
        h.has(s)
    }
    assert!(h.output.text.borrow().contains("38;2;134;227;181;48;2;28;31;36mSaturator"));
    h.emit(json!({"type":"focus","focus":focus}));
    let mut changed = focus.clone();
    changed["device"] = json!("EQ Eight");
    h.emit(json!({"type":"focus","focus":changed}));
    delay(5).await;
    changed["chain"] = json!("Blah");
    h.emit(json!({"type":"focus","focus":changed}));
    delay(5).await;
    assert_eq!(h.calls().iter().filter(|s| s.starts_with("tree:")).count(), 3);
    h.emit(json!({"type":"focus","focus":focus}));
    delay(5).await;
    let lines = h.screen();
    let row = lines.iter().position(|s| s.contains("│ │ ├ ≈  Saturator")).unwrap();
    h.type_text(&click(&lines, row, "Saturator")).await;
    h.has("≈  Audio Effect Rack › Chain 1 › Saturator  ×");
    assert!(h.screen()[row].trim_end().ends_with("pinned"));
    h.type_text("make it gentler\r").await;
    assert_eq!(
        serde_json::to_value(c.pins.borrow().last().unwrap()).unwrap(),
        json!({"trackRef":"3:track:3","ref":"d2a","node":"device","name":"Saturator","trail":["Audio Effect Rack","Chain 1"],"siblings":["EQ Eight"],"track":"4-Audio"})
    );
    h.type_text("\x1b").await;
    h.wait_until_hidden("Chain 1 › Saturator  ×").await;
    assert!(!has(&h.screen(), "Chain 1 › Saturator  ×"));
    h.type_text("\t").await;
    h.type_text("\x1b[B").await;
    h.type_text("\r").await;
    h.has("≈  Audio Effect Rack › Chain 1 › EQ Eight  ×");
    h.type_text("and brighter\r").await;
    assert_eq!(c.pins.borrow().last().unwrap().as_ref().unwrap().name, "EQ Eight");
    let lines = h.screen();
    let clear = lines.iter().position(|s| s.contains("EQ Eight  ×")).unwrap();
    h.type_text(&click(&lines, clear, "×")).await;
    assert!(!has(&h.screen(), "EQ Eight  ×"));
    h.close().await;
});
case!(clip_session_arrangement_and_fractional_scene_forwarding, async {
    let c = Rc::new(Control::default());
    *c.clip.borrow_mut()=Some(serde_json::from_value(json!({"slotRef":"3:clip_slot:2:0","name":"Chords","length":4,"notes":[{"pitch":60,"start":0,"duration":1,"velocity":90},{"pitch":64,"start":1,"duration":1,"velocity":90,"selected":true},{"pitch":67,"start":2,"duration":2,"velocity":90}]})).unwrap());
    let h = Harness::with(120, 36, c.clone(), |_| {});
    h.start().await;
    h.connect();
    let focus = json!({"track":{"name":"Keys","color":"#5ec1f7","kind":"midi"},"slotRef":"3:clip_slot:2:0","clip":"Chords","detail":"Clip","view":"Session","selectedNotes":1});
    h.emit(json!({"type":"focus","focus":focus}));
    delay(5).await;
    for s in ["FOCUS · Clip", "■  Keys › ▬  Chords", "1 bar · 3 notes · 1 selected"] {
        h.has(s)
    }
    assert!(h.screen().iter().filter(|s| s.chars().skip(80).any(|c| ('\u{2801}'..='\u{28ff}').contains(&c))).count() >= 2);
    assert!(h.output.text.borrow().contains("38;2;134;227;181;48;2;20;22;26m"));
    h.emit(json!({"type":"focus","focus":focus}));
    let mut changed = focus;
    changed["selectedNotes"] = json!(2);
    h.emit(json!({"type":"focus","focus":changed}));
    delay(5).await;
    assert_eq!(h.calls().iter().filter(|s| s.starts_with("clip:")).count(), 2);
    h.close().await;
    let c = Rc::new(Control::default());
    *c.session.borrow_mut()=Some(serde_json::from_value(json!({"trackRef":"3:track:1","scene":2,"slots":[{"index":0,"clip":{"name":"Intro","audio":false}},{"index":1,"clip":{"name":"Verse","audio":false},"playing":true},{"index":2,"clip":{"name":"Drop","audio":true},"queued":true},{"index":3}]})).unwrap());
    *c.arrangement.borrow_mut()=Some(serde_json::from_value(json!({"length":128,"position":64,"playing":true,"loop":{"start":64,"length":16,"enabled":true},"locators":[{"name":"Verse","position":32},{"name":"Drop","position":96}]})).unwrap());
    let h = Harness::with(120, 36, c, |_| {});
    h.start().await;
    h.connect();
    let mut focus = json!({"track":{"name":"Bass","color":"#f59a3c","kind":"midi"},"trackRef":"3:track:1","sceneIndex":2,"view":"Session"});
    h.emit(json!({"type":"focus","focus":focus}));
    delay(5).await;
    for s in ["FOCUS · Session", "■  Bass", "1 ▬  Intro", "2 ▬  Verse", "3 ▬  Drop", "4 ·"] {
        h.has(s)
    }
    let lines = h.screen();
    assert!(lines.iter().find(|s| s.contains("Verse")).unwrap().trim_end().ends_with("playing"));
    assert!(lines.iter().find(|s| s.contains("Drop")).unwrap().trim_end().ends_with("queued"));
    focus["view"] = json!("Arrangement");
    h.emit(json!({"type":"focus","focus":focus}));
    delay(5).await;
    for s in ["FOCUS · Arrangement", "bar 17 · playing · after Verse", "loop 17–21 · 32 bars"] {
        h.has(s)
    }
    assert!(regex::Regex::new("─+┼─+┃━+.*┼─+").unwrap().is_match(&h.screen().join("\n")));
    assert!(h.calls().contains(&"session:2".into()));
    assert!(h.calls().contains(&"arrangement".into()));
    focus["view"] = json!("Session");
    focus["sceneIndex"] = json!(2.5);
    h.emit(json!({"type":"focus","focus":focus}));
    delay(5).await;
    assert!(h.calls().contains(&"session:2.5".into()));
    focus["view"] = json!("Arrangement");
    h.emit(json!({"type":"focus","focus":focus}));
    focus["detail"] = json!("Device");
    focus["device"] = json!("Operator");
    h.emit(json!({"type":"focus","focus":focus}));
    assert!(!has(&h.screen(), "FOCUS · Arrangement"));
    assert!(!has(&h.screen(), "FOCUS · Session"));
    h.close().await;
});
/// What a pane's timer read again, by kind: `arrangement` calls.
fn arrangement_reads(h: &Harness) -> usize {
    h.calls().iter().filter(|c| *c == "arrangement").count()
}
fn arrangement_strip() -> Rc<Control> {
    let c = Rc::new(Control::default());
    *c.arrangement.borrow_mut() = Some(
        serde_json::from_value(
            json!({"length":128,"position":64,"playing":true,"loop":{"start":64,"length":16,"enabled":true},"locators":[]}),
        )
        .unwrap(),
    );
    c
}
case!(the_arrangement_shown_beside_a_clip_in_detail_is_what_the_pane_reads_again, async {
    tokio::time::pause();
    let h = Harness::with(120, 36, arrangement_strip(), |_| {});
    h.start().await;
    h.connect();
    // A clip in Live's detail view, then the Arrangement: the pane shows the Arrangement, not the clip.
    let mut focus = json!({"track":{"name":"Keys","color":"#5ec1f7","kind":"midi"},"trackRef":"3:track:1","sceneIndex":2,
        "slotRef":"3:clip_slot:1:2","clip":"Chords","detail":"Clip","view":"Session"});
    h.emit(json!({"type":"focus","focus":focus}));
    focus["view"] = json!("Arrangement");
    h.emit(json!({"type":"focus","focus":focus}));
    delay(5).await;
    h.has("FOCUS · Arrangement");
    let read = arrangement_reads(&h);
    tokio::time::advance(std::time::Duration::from_millis(3100)).await;
    delay(5).await;
    assert!(arrangement_reads(&h) > read, "the Arrangement shown was read again: {:?}", h.calls());
    h.close().await;
});
case!(the_pane_stops_reading_live_while_it_isnt_drawn, async {
    tokio::time::pause();
    let h = Harness::with(120, 36, arrangement_strip(), |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"focus","focus":{"track":{"name":"Keys","color":"#5ec1f7","kind":"midi"},"trackRef":"3:track:1","view":"Arrangement"}}));
    delay(5).await;
    h.has("FOCUS · Arrangement");
    tokio::time::advance(std::time::Duration::from_millis(1600)).await;
    delay(5).await;
    let read = arrangement_reads(&h);
    assert!(read >= 2, "the shown pane is kept fresh: {:?}", h.calls());
    // Narrower than the pane needs: the dock instead, and nothing to keep fresh.
    h.resize(90, 36);
    delay(5).await;
    tokio::time::advance(std::time::Duration::from_millis(4600)).await;
    delay(5).await;
    assert_eq!(arrangement_reads(&h), read, "{:?}", h.calls());
    h.close().await;
});
case!(pointed_live_pin_transport_and_interrupted_step, async {
    let h = Harness::new(120, 40);
    h.start().await;
    h.connect();
    let pin = json!({"trackRef":"3:track:0","ref":"3:arrangement_clip:0:0","node":"clip","name":"Verse riff","trail":["Bass"],"siblings":[],"live":true,"track":"Bass"});
    h.emit(json!({"type":"pointed","pin":pin}));
    h.has("Bass › Verse riff  ×");
    h.type_text("double it\r").await;
    assert_eq!(serde_json::to_value(h.control.pins.borrow().last().unwrap()).unwrap(), pin);
    assert!(!has(&h.screen(), "BPM"));
    h.emit(
        json!({"type":"transport","transport":{"playing":true,"tempo":120,"beat":8,"at":kumi_common::time::perf_now(),"beatsPerBar":4}}),
    );
    assert!(h.screen()[0].ends_with("● 120 BPM   ● Live  "));
    assert!(h.output.text.borrow().contains("38;2;255;225;77;48;2;14;15;18m●"));
    let before = h.output.text.borrow().len();
    h.emit(
        json!({"type":"transport","transport":{"playing":true,"tempo":120,"beat":8.5,"at":kumi_common::time::perf_now(),"beatsPerBar":4}}),
    );
    h.screen();
    assert!(h.output.text.borrow()[before..].contains("38;2;77;68;32;48;2;14;15;18m●"));
    h.emit(json!({"type":"transport","transport":{"playing":true,"tempo":123.5,"beat":9,"at":kumi_common::time::perf_now()}}));
    h.has("● 123.5 BPM");
    h.emit(json!({"type":"transport","transport":{"playing":false,"tempo":123.5,"beat":9.2,"at":kumi_common::time::perf_now()}}));
    assert!(!has(&h.screen(), "BPM"));
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"tool-start","id":"t1","name":"listen"}));
    h.emit(json!({"type":"doing","text":"hearing bar 17"}));
    h.has("listening · hearing bar 17");
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"cancelled"},"elapsedMs":900}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.has("│ × listened");
    assert!(!has(&h.screen(), "listening"));
    h.close().await;
});
case!(input_history_new_conversation_and_frame_budget, async {
    use kumi::history::open_input_history;
    use std::cell::RefCell;
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("input-history");
    let history = Rc::new(RefCell::new(open_input_history(Some(file.clone()), vec!["private-token".into()])));
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.history = Some(history));
    h.start().await;
    h.connect();
    h.type_text("make the bass wider\r").await;
    h.emit(json!({"type":"change","change":{"id":"c1","family":"mixer","title":"Bass width 100% → 140%","state":"applied","at":1}}));
    h.type_text("/new\r").await;
    h.has("make the bass wider");
    h.has("New conversation. Kumi won't use what's above");
    assert!(h.screen().iter().any(|s| s.contains("Bass width") && s.contains("undo")));
    h.type_text("use my key sk-ant-api03-Abcdefghijklmnopqrstuvwxyz0123456789 and private-token\r").await;
    h.type_text("half-typed").await;
    for (key, text) in [
        ("\x1b[A", "use my key [redacted] and [redacted]"),
        ("\x1b[A", "/new"),
        ("\x1b[A", "make the bass wider"),
        ("\x1b[A", "make the bass wider"),
        ("\x1b[B", "/new"),
        ("\x1b[B\x1b[B", "half-typed"),
    ] {
        h.type_text(key).await;
        let lines = h.screen();
        assert!(has(&lines[lines.len() - 4..], text));
    }
    h.close().await;
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(!saved.contains("sk-ant"));
    assert!(!saved.contains("private-token"));
    assert_eq!(open_input_history(Some(file), vec![]).entries(), ["make the bass wider", "/new", "use my key [redacted] and [redacted]"]);
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    for i in 0..1000 {
        h.emit(json!({"type":"notice","message":format!("earlier note {i}")}));
        h.emit(json!({"type":"notice","message":format!("and another {i}")}));
    }
    h.type_text("go\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.screen();
    let before = h.output.text.borrow().matches("\x1b[?2026h").count();
    for i in 0..200 {
        h.emit(json!({"type":"text","text":format!("word{i} ")}));
    }
    h.screen();
    assert_eq!(h.output.text.borrow().matches("\x1b[?2026h").count() - before, 1);
    let before = h.app.transcript_layout_count();
    h.emit(json!({"type":"text","text":"one more"}));
    h.screen();
    assert_eq!(h.app.transcript_layout_count() - before, 1);
    h.close().await;
});
case!(whole_screens_match_source, async {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("support/app/screens.json")).unwrap();
    for case in cases {
        let h = Harness::new(case["width"].as_i64().unwrap() as i32, case["height"].as_i64().unwrap() as i32);
        h.start().await;
        for event in case["events"].as_array().unwrap() {
            h.emit(event.clone());
        }
        let actual = h.screen();
        let expected: Vec<String> = serde_json::from_value(case["expected"].clone()).unwrap();
        assert_eq!(actual, expected, "{} {}×{}", case["name"], case["width"], case["height"]);
        h.close().await;
    }
});

#[tokio::test(flavor = "current_thread")]
async fn quitting_right_after_a_change_leaves_nothing_running() {
    // A change's flash ends with a redraw 4 s later. Quitting before then mustn't wait on it: the
    // process's shutdown waits for every task, then calls a leftover a live handle and exits 1.
    let local = tokio::task::LocalSet::new();
    let code = local
        .run_until(async {
            let h = Harness::new(120, 36);
            let done = h.app.run();
            delay(5).await;
            h.connect();
            h.emit(json!({"type":"remembered","scope":"set","note":{"id":"s1","text":"The Reese is the main bass","at":1}}));
            delay(5).await;
            h.type_text("\x03").await;
            done.await
        })
        .await;
    assert_eq!(code, 0);
    assert!(tokio::time::timeout(std::time::Duration::from_millis(500), local).await.is_ok(), "a task outlived the app");
}
case!(a_question_with_numbered_options_answers_by_number_and_free_text_still_works, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    let ask = |h: &Harness| {
        h.emit(json!({"type":"state","state":"running"}));
        h.emit(json!({"type":"text","text":"Which bass should duck under the kick?\n\n1. **Sub Bass**\n2. Reese\n3. Both"}));
        h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
        h.emit(json!({"type":"state","state":"idle"}));
    };
    h.type_text("sidechain the bass\r").await;
    ask(&h);
    h.has("Your answer");
    h.has("2. Reese");
    h.has("a number, then enter answers");
    h.type_text("2").await;
    assert!(!h.calls().iter().any(|c| c == "submit:Reese"), "a number picks; it doesn't send");
    h.type_text("\r").await;
    h.wait_for_call("submit:Reese").await;
    // The producer's pick, with the question and its options, to learn their taste from.
    assert!(h.calls().iter().any(|c| c == "picked:answer:1 of 3:Which bass should duck under the kick?"), "{:?}", h.calls());
    assert!(!has(&h.screen(), "Your answer"), "answering closes the choices");
    ask(&h);
    h.type_text("2 dB quieter, keep both").await;
    assert!(!has(&h.screen(), "Your answer"), "typing goes to the input box");
    h.has("2 dB quieter, keep both");
    assert_eq!(h.calls().iter().filter(|c| c.starts_with("submit:")).count(), 2, "nothing more was sent");
    h.type_text("\x03").await;
    assert!(!has(&h.screen(), "keep both"));
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Done:\n1. Sidechained Reese\n2. Lowered the sub 2 dB"}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
    h.emit(json!({"type":"state","state":"idle"}));
    assert!(!has(&h.screen(), "Your answer"), "a list that isn't a question offers nothing");
    h.close().await;
});
case!(a_double_enter_on_the_answers_sends_the_answer_once, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("sidechain the bass\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Which bass should duck under the kick?\n\n1. Sub Bass\n2. Reese"}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.has("Your answer");
    // Both Enters arrive before the first choice is made.
    h.type_text("2\r\r").await;
    h.wait_for_call("submit:Reese").await;
    delay(20).await;
    assert_eq!(h.calls().iter().filter(|c| *c == "submit:Reese").count(), 1, "{:?}", h.calls());
    h.close().await;
});
case!(only_a_choice_the_producer_made_is_a_pick, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    let ask = |h: &Harness| {
        h.emit(json!({"type":"state","state":"running"}));
        h.emit(json!({"type":"text","text":"Which pad?\n\n1. Warm\n2. Glassy"}));
        h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
        h.emit(json!({"type":"state","state":"idle"}));
    };
    let picks = |h: &Harness| h.calls().into_iter().filter(|c| c.starts_with("picked:")).collect::<Vec<_>>();
    // Enter on the first option, untouched, sends it but says nothing of taste.
    h.type_text("make a pad\r").await;
    ask(&h);
    h.has("Your answer");
    h.type_text("\r").await;
    h.wait_for_call("submit:Warm").await;
    assert!(picks(&h).is_empty(), "{:?}", picks(&h));
    // Moving to an option is a choice.
    ask(&h);
    h.has("Your answer");
    h.type_text("\x1b[B\r").await;
    h.wait_for_call("submit:Glassy").await;
    assert_eq!(picks(&h), ["picked:answer:1 of 2:Which pad?"]);
    h.close().await;
});
case!(a_provider_wait_shows_why_and_counts_down_until_the_model_answers, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    h.type_text("make it louder\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"retry","reason":"ChatGPT is busy (HTTP 429)","waitMs":4200}));
    h.has("retrying in 5s · ChatGPT is busy");
    h.has("esc to stop");
    h.emit(json!({"type":"text","text":"Raised the master 2 dB."}));
    assert!(!has(&h.screen(), "retrying in"), "the answer ends the wait");
    // An answer that broke off carries on at once: no countdown, until the model's next words.
    h.emit(json!({"type":"retry","reason":"ChatGPT's answer broke off","waitMs":0}));
    h.has("carrying on · ChatGPT's answer");
    h.emit(json!({"type":"text","text":" Then the low end."}));
    assert!(!has(&h.screen(), "carrying on"), "the next words end it");
    h.close().await;
});
case!(whats_new_shows_once_as_kumi_starts_below_a_conversation_carried_on_and_changelog_has_the_rest, async {
    let news = kumi::whats_new::News {
        title: "What's new in Kumi 1.8.12, since 1.8.10".into(),
        items: (1..=5).map(|n| format!("1.8.12 · Note number {n} about something Kumi does better now.")).collect(),
        more: 3,
        since: Some("1.8.10".into()),
    };
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.whats_new = Some(news));
    h.start().await;
    h.connect();
    h.has("What's new in Kumi 1.8.12, since 1.8.10");
    h.has("· 1.8.12 · Note number 1 about something Kumi does better now.");
    h.has("and 3 more since 1.8.10: /changelog");
    // A conversation carried on at the start goes above it, so the notes stay in view.
    h.emit(json!({"type":"resumed","savedAt":kumi_common::time::now_ms()-3600000,"lines":[{"role":"user","text":"make the bass wider"},{"role":"assistant","text":"Widened it to 140%."}],"changes":[]}));
    let lines = h.screen();
    let widened = lines.iter().position(|s| s.contains("Widened it to 140%.")).unwrap();
    let title = lines.iter().position(|s| s.contains("What's new in Kumi 1.8.12")).unwrap();
    assert!(widened < title, "{lines:#?}");
    // /changelog shows the notes from the real changelog, with where the rest are.
    h.type_text("/changelog\r").await;
    h.has("Every release: https://github.com/user1303836/kumi/blob/main/CHANGELOG.md");
    h.close().await;
});
