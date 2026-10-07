#[path = "support/models.rs"]
mod models;
#[path = "support/tui_app.rs"]
mod support;
use futures::FutureExt;
use kumi::{terminal::Terminal, tui::app::PanelTab};
use models::FakeModels;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}
fn model_harness(m: Rc<FakeModels>, width: i32) -> Harness {
    Harness::with(width, 40, Rc::new(Control::default()), |o| o.models = Some(m))
}
case!(missing_default_model_and_commands_over_picker, async {
    let fake = FakeModels::catalog();
    let h = model_harness(fake, 120);
    h.start().await;
    h.emit(json!({"type":"error","message":"ChatGPT doesn't offer gpt-6-astra to this sign-in (HTTP 404); choose another model.","kind":"model","provider":"openai-codex"}));
    h.has("Choose another model?");
    h.type_text("\r").await;
    h.has("Choose a model");
    h.close().await;
    let fake = FakeModels::catalog();
    *fake.model.borrow_mut() = None;
    let h = model_harness(fake, 120);
    h.start().await;
    h.has("Kumi talks to GPT-6 Astra, ChatGPT's first choice. /model changes it.");
    h.close().await;
    let fake = FakeModels::catalog();
    *fake.model.borrow_mut() = None;
    fake.signed_in.borrow_mut().clear();
    let h = model_harness(fake, 120);
    h.start().await;
    for s in
        ["Sign in to a provider to talk to its models", "Choose a model", "Sign in to ChatGPT", "with your ChatGPT plan", "no model chosen"]
    {
        h.has(s)
    }
    h.type_text("/").await;
    assert!(!has(&h.screen(), "Choose a model"));
    h.has("Forget this conversation and start fresh");
    h.type_text("help\r").await;
    h.has("enter sends · ctrl+j or alt+enter starts a new line");
    h.type_text("/model\r").await;
    h.type_text("gpt-6/").await;
    h.has("filter: gpt-6/");
    h.close().await;
});
case!(chatgpt_browser_clipboard_success_and_cancel, async {
    let fake = FakeModels::catalog();
    *fake.model.borrow_mut() = None;
    fake.signed_in.borrow_mut().clear();
    let h = model_harness(fake.clone(), 120);
    h.start().await;
    h.type_text("\r").await;
    for s in
        ["Sign in to ChatGPT", "https://auth.example.test/oauth/authorize", "Waiting for the browser…", "c copies the link · esc to cancel"]
    {
        h.has(s)
    }
    let url = "https://auth.example.test/oauth/authorize?client=kumi&state=fixture";
    assert_eq!(*h.browsed.borrow(), [url]);
    h.type_text("c").await;
    use base64::Engine;
    assert!(h.output.text.borrow().contains(&format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(url))));
    h.has("Copied the sign-in link.");
    fake.finish_chatgpt.borrow_mut().take().unwrap().send(()).unwrap();
    delay(10).await;
    h.has("Signed in to ChatGPT.");
    assert!(h.screen().iter().any(|s| s.contains("GPT-6 Astra") && s.contains("current")));
    h.type_text("\x1b").await;
    h.wait_until_hidden("Choose a model").await;
    h.type_text("/login\r").await;
    h.type_text("\r").await;
    h.has("Waiting for the browser…");
    h.type_text("\x1b").await;
    h.wait_until_hidden("Waiting for the browser…").await;
    assert!(!has(&h.screen(), "Waiting for the browser…"));
    assert!(!has(&h.screen(), "didn't finish"));
    h.close().await;
});
fn local_models() -> Rc<FakeModels> {
    let m = FakeModels::catalog();
    m.lists.borrow_mut().insert("ollama".into(),serde_json::from_value(json!([{ "id":"ollama/qwen3:8b","provider":"ollama","model":"qwen3:8b","name":"qwen3:8b","description":"8.2B · Q4_K_M · loaded","efforts":[],"tools":true,"loaded":true,"where":"on this computer"},{"id":"ollama/gemma3:4b","provider":"ollama","model":"gemma3:4b","name":"gemma3:4b","description":"4.3B · Q4_K_M · can't change the Set","efforts":[],"tools":false,"where":"on this computer"}])).unwrap());
    *m.local.borrow_mut() = vec![
        kumi::models::LocalStatus {
            id: "ollama".into(),
            name: "Ollama".into(),
            r#where: "on this computer".into(),
            running: true,
            start: None,
        },
        kumi::models::LocalStatus {
            id: "lmstudio".into(),
            name: "LM Studio".into(),
            r#where: "on this computer".into(),
            running: false,
            start: Some("Open LM Studio and start its server (Developer tab), or run: lms server start".into()),
        },
    ];
    m.notes.borrow_mut().insert("ollama/gemma3:4b".into(),"gemma3:4b can't use tools, so Kumi can talk with it about your Set but can't change anything. qwen3:8b on Ollama can: /model chooses it.".into());
    m
}
case!(local_models_choices_default_network_retry_and_closed_startup, async {
    let fake = local_models();
    let h = model_harness(fake.clone(), 140);
    h.start().await;
    h.type_text("/model\r").await;
    h.type_text("computer").await;
    for s in [
        "Ollama · on this computer",
        "8.2B · Q4_K_M · loaded",
        "can't change the Set",
        "LM Studio · on this computer",
        "not running",
        "Open LM Studio and start its server",
    ] {
        h.has(s)
    }
    assert!(!has(&h.screen(), "Sign in to LM Studio"));
    assert!(!has(&h.screen(), "Sign in to Ollama"));
    h.type_text("\x1b").await;
    h.wait_until_hidden("Choose a model").await;
    h.type_text("/model\r").await;
    h.type_text("gemma\r").await;
    assert!(fake.calls.borrow().contains(&"choose:ollama/gemma3:4b".into()));
    h.has("Kumi talks to gemma3:4b from your next message.");
    h.has("can't use tools");
    assert!(h.screen()[0].contains("gemma3:4b"));
    h.close().await;
    let fake = local_models();
    *fake.model.borrow_mut() = None;
    fake.signed_in.borrow_mut().clear();
    let h = model_harness(fake, 140);
    h.start().await;
    h.has("Kumi talks to qwen3:8b, in Ollama on this computer. /model changes it.");
    assert!(!has(&h.screen(), "Choose a model"));
    h.type_text("Tighten the kick\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"error","message":"Ollama isn't running: open it, or run `ollama serve`, then send your message again.","kind":"network","provider":"ollama"}));
    h.emit(json!({"type":"state","state":"idle"}));
    for s in ["Ollama isn't running", "Send your message again?", "Send it again", "once Ollama is running"] {
        h.has(s)
    }
    h.type_text("\r").await;
    assert_eq!(h.calls().iter().filter(|s| s.starts_with("submit:")).count(), 2);
    h.close().await;
    let fake = local_models();
    *fake.model.borrow_mut() = Some("ollama/qwen3:8b".into());
    fake.local.borrow_mut()[0].running = false;
    fake.local.borrow_mut()[0].start = Some("Open Ollama, or run: ollama serve".into());
    let h = model_harness(fake, 140);
    h.start().await;
    h.has("Ollama isn't running, so qwen3:8b can't answer yet. Open Ollama, or run: ollama serve.");
    assert!(!has(&h.screen(), "Sign in to"));
    h.close().await;
});
case!(update_offers_checks_and_closes_after_confirmation, async {
    let requested = Rc::new(Cell::new(0));
    let unreachable = Rc::new(Cell::new(false));
    let updates = kumi::update::UpdateControl {
        current: "1.0.0".into(),
        request: {
            let r = requested.clone();
            Rc::new(move || r.set(r.get() + 1))
        },
        check: {
            let u = unreachable.clone();
            Rc::new(move || {
                let u = u.clone();
                async move {
                    if u.get() {
                        Err(kumi_runtime::core::errors::RuntimeError::plain(
                            "Kumi couldn't reach GitHub to ask; check your internet connection",
                        ))
                    } else {
                        Ok(None)
                    }
                }
                .boxed_local()
            })
        },
    };
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.updates = Some(updates.clone()));
    h.start().await;
    h.connect();
    h.type_text("/update\r").await;
    h.has("Kumi is up to date (1.0.0).");
    unreachable.set(true);
    h.type_text("/update\r").await;
    h.has("Kumi couldn't reach GitHub to ask");
    h.has("/update again later.");
    h.app.offer_update("1.1.0");
    h.has("Kumi 1.1.0 is out: /update gets it.");
    h.close().await;
    assert_eq!(requested.get(), 0);
    unreachable.set(false);
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.updates = Some(updates));
    let done = h.app.run();
    delay(5).await;
    h.connect();
    h.app.offer_update("1.1.0");
    h.has("Kumi 1.1.0 is out · /update gets it");
    h.has("Kumi can see Night Drive.");
    h.type_text("/update\r").await;
    h.has("Update to Kumi 1.1.0?");
    h.has("Kumi closes, updates and opens again");
    h.type_text("\x1b[B\r").await;
    assert!(!has(&h.screen(), "Update to Kumi 1.1.0?"));
    assert_eq!(requested.get(), 0);
    h.type_text("/update\r").await;
    h.type_text("\r").await;
    assert_eq!(done.await, 0);
    assert_eq!(requested.get(), 1);
    h.close().await;
    let h = Harness::new(120, 36);
    h.start().await;
    h.type_text("/upd").await;
    assert!(!has(&h.screen(), "Get the newest Kumi"));
    h.close().await;
});
case!(memory_notes_techniques_recipes_and_taste, async {
    let c = Rc::new(Control::default());
    *c.memory.borrow_mut()=Some(serde_json::from_value(json!({"producer":[{"id":"p1","text":"Prefers short, dark reverbs","at":1}],"set":[{"id":"s1","text":"The Reese is the main bass","at":2}],"setName":"Night Drive","saved":true})).unwrap());
    c.set("forgot", json!({"id":"s1","text":"The Reese is the main bass","at":2}));
    let h = Harness::with(120, 36, c.clone(), |_| {});
    h.start().await;
    h.connect();
    h.type_text("/memory\r").await;
    for s in ["What Kumi remembers", "About you", "Prefers short, dark reverbs", "About Night Drive", "The Reese is the main bass"] {
        h.has(s)
    }
    // A note: change its words in the box, pin it, or forget it, without the model.
    h.type_text("\x1b[B\r").await;
    for s in ["“The Reese is the main bass”", "Change the words", "Pin it", "A full memory never drops it to make room", "Forget it"] {
        h.has(s)
    }
    h.type_text("\x1b[B\r").await;
    assert!(h.calls().contains(&"change-note:s1 Pinned(true)".into()));
    h.has("Pinned: Kumi keeps this note even when its memory is full.");
    h.type_text("/memory\r").await;
    h.has("pinned ·");
    h.type_text("\r").await;
    h.has("“Prefers short, dark reverbs”");
    h.type_text("\r").await;
    h.has("/note p1 Prefers short, dark reverbs");
    h.type_text(" on drums\r").await;
    assert!(h.calls().contains(&"change-note:p1 Text(\"Prefers short, dark reverbs on drums\")".into()));
    h.has("Changed note p1.");
    h.type_text("/note p1\r").await;
    h.has("Give the note's id and its new words");
    h.type_text("\x15/memory\r").await;
    h.type_text("\x1b[B\r").await;
    h.type_text("\x1b[B\x1b[B\r").await;
    assert!(h.calls().contains(&"forget:s1".into()));
    assert!(!has(&h.screen(), "Change the words"));
    h.close().await;
    c.set("techniques", json!([{"id":"t1","name":"Neuro from a Reese","fits":"gritty, moving neuro basses","source":"Au5 · Neuro bass"}]));
    c.set("recipes", json!([{"name":"Drum bus","about":"a return with glue compression","params":[],"steps":3,"used":0,"created":1}]));
    let h = Harness::with(120, 36, c.clone(), |_| {});
    h.start().await;
    h.connect();
    h.type_text("/memory\r").await;
    for s in ["What Kumi remembers · notes, techniques and recipes", "Techniques", "Neuro from a Reese", "Recipes", "Drum bus"] {
        h.has(s)
    }
    h.type_text("techn\r").await;
    h.has("Forget this technique?");
    h.type_text("\r").await;
    assert!(h.calls().contains(&"forget-technique:t1".into()));
    h.close().await;
    let c = Rc::new(Control::default());
    *c.memory.borrow_mut() = Some(serde_json::from_value(json!({"producer":[],"set":[],"saved":true,"setName":"Night Drive"})).unwrap());
    *c.library.borrow_mut() =
        Some(serde_json::from_value(json!({"state":"learning","sounds":120,"presets":40,"sets":3,"todo":900,"done":120})).unwrap());
    c.set("taste",json!([{"id":"chain-vocal","line":"Vocals: EQ Eight → Compressor → Reverb (on 4 of 4 vocal tracks)"},{"id":"tempo","line":"Tempo: usually 124–126 BPM"}]));
    let h = Harness::with(240, 36, c, |_| {});
    h.start().await;
    h.connect();
    h.has("Learning your library in the background · 120 of 900 sounds");
    *h.control.library.borrow_mut() =
        Some(serde_json::from_value(json!({"state":"ready","sounds":1020,"presets":40,"sets":3,"learnedAt":1})).unwrap());
    h.emit(json!({"type":"library","status":{"state":"ready","sounds":1020,"presets":40,"sets":3,"learnedAt":1}}));
    h.has("Your library: 1,020 sounds · 40 presets · 3 Sets");
    h.type_text("/status\r").await;
    h.has("· Your library: 1,020 sounds · 40 presets · 3 Sets");
    h.type_text("/memory\r").await;
    for s in ["From your Sets", "Vocals: EQ Eight → Compressor → Reverb (on 4 of 4 vocal tracks)", "Tempo: usually 124–126 BPM"] {
        h.has(s)
    }
    h.type_text("vocals\r").await;
    h.has("Forget this, from your Sets?");
    h.type_text("\r").await;
    assert!(h.calls().contains(&"forget-taste:chain-vocal".into()));
    h.has("Forgot, from your Sets: Vocals: EQ Eight → Compressor → Reverb");
    h.close().await;
    let h = Harness::new(160, 36);
    h.start().await;
    h.connect();
    h.emit(json!({"type":"notice","message":"Continuing your conversation from 2 hours ago. /new starts fresh."}));
    for done in [0, 1] {
        h.emit(json!({"type":"library","status":{"state":"learning","sounds":done,"presets":0,"sets":0,"todo":10,"done":done}}));
    }
    assert_eq!(h.screen().iter().filter(|s| s.contains("Learning your library in the background…")).count(), 1);
    h.close().await;
});
case!(recipes_run_and_fill_blanks, async {
    let c = Rc::new(Control::default());
    c.set("recipes",json!([{"name":"Drum bus","about":"Glue, saturation and a short room on a new return","params":[],"steps":3,"used":2,"lastUsed":kumi_common::time::now_ms()-86400000,"created":1},{"name":"Resample twice","about":"OTT and Saturator, then Grain Delay","params":[{"name":"track","about":"the track to resample"}],"steps":6,"used":0,"created":2}]));
    c.set("recipe-result", json!({"text":"Done:\n- Added return track “Drum Bus”","isError":false}));
    let h = Harness::with(120, 36, c, |_| {});
    h.start().await;
    h.connect();
    h.type_text("/recipes\r").await;
    for s in ["Your recipes", "Drum bus", "Resample twice", "used 1 day ago"] {
        h.has(s)
    }
    h.type_text("\r").await;
    h.has("Run it now");
    h.type_text("\r").await;
    assert!(h.calls().contains(&"run-recipe:Drum bus".into()));
    h.has("Added return track “Drum Bus”");
    h.type_text("/recipes\r").await;
    h.type_text("\x1b[B\r").await;
    h.has("Run it on…");
    h.has("Kumi needs: the track to resample");
    h.type_text("\r").await;
    // Its blanks go on a /recipe line; one left empty is named, and nothing runs.
    h.has("/recipe \"Resample twice\" track=");
    h.type_text("\r").await;
    h.has("“Resample twice” needs track (the track to resample).");
    let resampled = |calls: Vec<String>| calls.into_iter().filter(|c| c.starts_with("run-recipe:Resample")).collect::<Vec<_>>();
    assert!(resampled(h.calls()).is_empty());
    // Filled in, it runs straight away: no model call.
    h.type_text("3:track:2\r").await;
    assert_eq!(resampled(h.calls()), [r#"run-recipe:Resample twice {"track":"3:track:2"}"#]);
    assert!(!h.calls().iter().any(|c| c.starts_with("submit")));
    h.type_text("/recipe \"resample twice\" track=\"My \\\"Bass\\\"\"\r").await;
    assert_eq!(resampled(h.calls())[1], r#"run-recipe:Resample twice {"track":"My \"Bass\""}"#);
    // Unquoted, a number goes as a number, as the model would pass it; in quotes, as words.
    h.type_text("/recipe \"resample twice\" track=124\r").await;
    assert_eq!(resampled(h.calls())[2], r#"run-recipe:Resample twice {"track":124}"#);
    h.type_text("/recipe \"resample twice\" track=\"124\"\r").await;
    assert_eq!(resampled(h.calls())[3], r#"run-recipe:Resample twice {"track":"124"}"#);
    for (line, says) in [
        ("/recipe", "Run a recipe with: /recipe <name> blank=value"),
        ("/recipe \"Resample twice track=1", "Run a recipe with: /recipe <name> blank=value"),
        ("/recipe Gone", "Kumi keeps no recipe called “Gone”"),
        ("/recipe \"Resample twice\" track=1 amount=2", "“Resample twice” has no blank called amount; its blanks are track."),
        ("/recipe \"Drum bus\" track=1", "“Drum bus” has no blanks to fill: /recipe \"Drum bus\" runs it."),
    ] {
        h.type_text(&format!("{line}\r")).await;
        h.has(says);
        h.type_text("\x15").await;
    }
    assert_eq!(resampled(h.calls()).len(), 4);
    // What's pinned fills the blank named for it.
    h.emit(json!({"type":"pointed","pin":{"trackRef":"3:track:1","ref":"3:track:1","node":"track","name":"Bass","trail":[],"siblings":[],"live":true}}));
    h.type_text("/recipes\r").await;
    h.type_text("\x1b[B\r").await;
    h.type_text("\r").await;
    h.has("/recipe \"Resample twice\" track=3:track:1");
    h.emit(json!({"type":"recipe","action":"saved","name":"Vocal chain","steps":4}));
    h.has("↻ Saved a recipe: Vocal chain (4 steps)");
    h.close().await;
});
case!(a_goal_and_its_loop_show_where_they_are_and_each_judged_round, async {
    let c = Rc::new(Control::default());
    c.set("goal", json!(true));
    let h = Harness::with(140, 40, c, |_| {});
    h.start().await;
    h.connect();
    h.type_text("/goal master this to -9 LUFS\r").await;
    assert!(h.calls().contains(&"goal:master this to -9 LUFS".into()));
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"objective","objective":"master this to -9 LUFS","state":"running","turns":1,"turnBudget":12,"elapsedMs":65000,"budgetMs":3600000,"verdict":"continue","reason":"not met yet, by the model's own check","next":"raise the limiter","measured":false}));
    for s in [
        "working · turn 1 of 12 · 1:05 of 1:00:00",
        "checked: not met yet, by the model's own",
        "next: raise the limiter",
        "goal · turn 2/12",
    ] {
        h.has(s)
    }
    // The loop inside it: where it is, and each round in the transcript.
    h.emit(json!({"type":"loop","state":"running","request":"master this to -9 LUFS","rounds":1,"kept":1,"reverted":0,"listens":3,"elapsedMs":30000,"roundsLeft":15,"next":"True peak"}));
    h.has("loop · round 1 · 1 kept");
    h.emit(json!({"type":"judged","round":2,"kind":"judged","heard":"the mix, bars 49–57","target":"True peak","change":"Limiter ceiling -1 → -3.5 dB","rows":[],"kept":false,"why":"it hurt punch (crest)","met":false,"listens":4,"elapsedMs":42000}));
    for s in ["Round 2 · true peak", "change: Limiter ceiling -1 → -3.5 dB", "reverted: it hurt punch (crest)", "4 listens · 0:42"] {
        h.has(s)
    }
    h.emit(json!({"type":"loop","state":"done","request":"master this to -9 LUFS","rounds":2,"kept":1,"reverted":1,"listens":5,"elapsedMs":61000,"roundsLeft":14,"stop":"met"}));
    h.has("Loop: 2 rounds · 1 kept · 1 taken back · 5 listens · 1:01 · every item within tolerance");
    h.emit(json!({"type":"objective","objective":"master this to -9 LUFS","state":"done","turns":2,"turnBudget":12,"elapsedMs":90000,"budgetMs":3600000,"verdict":"complete","reason":"the judge's checklist is met","measured":true}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.has("met · turn 2 of 12");
    h.has("measured: the judge's checklist is met");
    h.close().await;
});
case!(goal_dashboard_and_aside_panel, async {
    let c = Rc::new(Control::default());
    c.set("goal", json!(true));
    let h = Harness::with(140, 40, c, |_| {});
    h.start().await;
    h.connect();
    h.type_text("/goal make my pad sound like ~/ref.wav\r").await;
    assert!(h.calls().contains(&"goal:make my pad sound like ~/ref.wav".into()));
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"goal","state":"running","goal":"make my pad sound like ~/ref.wav","generation":12,"rendered":36,"trend":[58,58,61,64,64,70,71,71,74,76,76,81],"first":58,"best":{"label":"Collision","score":81},"leader":"Collision · Collision → Delay → Limiter","idea":"Tried a Collision with parallel delays.","elapsedMs":185000,"candidates":3}));
    for s in [
        "GOAL",
        "searching · gen 12 · 36 heard · 3",
        "candidates · 3:05",
        "81% from 58%",
        "best  Collision · Collision → Delay →",
        "tried  Tried a Collision with parallel",
        "goal · 81% · gen 12 · 3:05",
    ] {
        h.has(s)
    }
    assert!(regex::Regex::new("▁.*█").unwrap().is_match(&h.screen().join("\n")));
    h.type_text("/goal stop\r").await;
    assert!(h.calls().contains(&"stop-goal".into()));
    h.type_text("/loop stop\r").await;
    assert!(h.calls().contains(&"stop-loop".into()));
    h.emit(json!({"type":"goal","state":"done","goal":"make my pad sound like ~/ref.wav","generation":13,"rendered":39,"trend":[58,81],"first":58,"best":{"label":"Collision","score":81},"elapsedMs":200000,"candidates":3,"bestTrack":"Kumi · Goal best","why":"stopped"}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.has("done · stopped · gen 13");
    h.has("kept on  Kumi · Goal best");
    assert!(!has(&h.screen(), "goal · 81%"));
    h.close().await;
    let c = Rc::new(Control::default());
    c.set("aside", json!("Erbe-Verb's tail runs **up to a minute**."));
    let release = kumi_common::abort::Signal::new();
    *c.aside_gate.borrow_mut() = Some(release.clone());
    let h = Harness::with(120, 36, c, |_| {});
    h.start().await;
    h.connect();
    h.type_text("build me a reverb\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"tool-start","id":"t1","name":"make_device"}));
    h.type_text("/btw how long can its tail be?\r").await;
    assert!(h.calls().contains(&"aside:how long can its tail be?".into()));
    for s in ["btw · how long can its tail be?", "Erbe-Verb's tail runs up to a minute.", "making a device"] {
        h.has(s)
    }
    release.cancel();
    delay(5).await;
    h.type_text("\x1b").await;
    h.wait_until_hidden("btw ·").await;
    assert!(!has(&h.screen(), "btw ·"));
    assert!(!has(&h.screen(), "up to a minute"));
    assert!(!h.calls().iter().any(|s| s.starts_with("submit:") && s.contains("tail")));
    h.type_text("/btw\r").await;
    h.has("up to a minute");
    h.close().await;
});
struct Stub;
impl kumi::tui::tabs::Tab for Stub {
    fn id(&self) -> &str {
        "stub"
    }
    fn title(&self) -> &str {
        "STUB"
    }
    fn rows(&self, _: i32) -> Rc<[kumi::tui::tabs::TabRow]> {
        Rc::new([kumi::tui::tabs::TabRow {
            spans: vec![kumi::tui::wrap::Span::styled("stub row", Default::default())],
            ..Default::default()
        }])
    }
}
fn strip(lines: &[String]) -> usize {
    lines.iter().position(|s| s.contains("HISTORY") && s.contains("───")).unwrap()
}
case!(tab_area_anchor_custom_tabs_and_remembered_selection, async {
    for rows in [24, 36, 50] {
        let h = Harness::new(120, rows);
        h.start().await;
        h.connect();
        let lines = h.screen();
        let pane = rows - 1;
        let bottom = (pane / 2).max(7).min(pane - 8);
        assert_eq!(strip(&lines) as i32, 1 + pane - bottom);
        assert!(lines.iter().position(|s| s.contains("NOW")).unwrap() < strip(&lines));
        h.has("Nothing changed yet");
        h.close().await;
    }
    let saved = Rc::new(RefCell::new(None::<String>));
    let options = |o: &mut kumi::tui::app::TuiOptions| {
        o.tabs = vec![Rc::new(Stub)];
        let s = saved.clone();
        let t = saved.clone();
        o.panel_tab =
            Some(PanelTab { load: Rc::new(move || s.borrow().clone()), save: Rc::new(move |id| *t.borrow_mut() = Some(id.into())) });
    };
    let h = Harness::with(120, 36, Rc::new(Control::default()), options);
    h.start().await;
    h.connect();
    let lines = h.screen();
    let row = strip(&lines);
    assert!(lines[row].contains("STUB"));
    h.type_text(&click(&lines, row, "STUB")).await;
    assert!(h.screen()[row + 1].contains("stub row"));
    assert_eq!(saved.borrow().as_deref(), Some("stub"));
    h.type_text("\x1b[Z").await;
    h.type_text("\x1b[Z").await;
    h.has("Nothing changed yet");
    assert_eq!(saved.borrow().as_deref(), Some("history"));
    h.close().await;
    *saved.borrow_mut() = Some("stub".into());
    let h = Harness::with(120, 36, Rc::new(Control::default()), options);
    h.start().await;
    h.connect();
    assert!(h.screen()[row + 1].contains("stub row"));
    h.close().await;
});
case!(usage_status_api_plan_and_conversation_picker, async {
    for keyed in [true, false] {
        let m = FakeModels::catalog();
        if keyed {
            *m.model.borrow_mut() = Some("anthropic/claude-sonnet-5-5".into());
            m.signed_in.borrow_mut().insert(kumi_runtime::providers::ProviderId::Anthropic);
        }
        let h = model_harness(m, 240);
        h.start().await;
        for usage in [
            json!({"inputTokens":9000,"outputTokens":700,"cacheReadTokens":6000,"cacheWriteTokens":0}),
            json!({"inputTokens":4500,"outputTokens":520,"cacheReadTokens":2000,"cacheWriteTokens":0}),
        ] {
            h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed","usage":usage},"elapsedMs":900}));
        }
        h.type_text("/status\r").await;
        if keyed {
            h.has("this session: 13.5k tokens in (8.0k cached), 1.2k out")
        } else {
            assert!(!has(&h.screen(), "tokens in"))
        }
        h.close().await;
    }
    let c = Rc::new(Control::default());
    *c.conversations.borrow_mut()=serde_json::from_value(json!([{"id":"now001","savedAt":kumi_common::time::now_ms(),"first":"add a hi-hat groove","turns":2,"current":true},{"id":"old001","savedAt":kumi_common::time::now_ms()-2*3600000,"first":"make the bass wider","turns":5,"current":false}])).unwrap();
    let h = Harness::with(120, 36, c, |_| {});
    h.start().await;
    h.connect();
    h.type_text("/conversations\r").await;
    h.has("Conversations about Night Drive");
    assert!(h.screen().iter().any(|s| s.contains("add a hi-hat groove") && s.contains("this one · 2 requests")));
    assert!(h.screen().iter().any(|s| s.contains("make the bass wider") && s.contains("2 hours ago · 5 requests")));
    h.type_text("\x1b[B\r").await;
    assert!(h.calls().contains(&"resume:old001".into()));
    h.emit(json!({"type":"resumed","savedAt":kumi_common::time::now_ms()-2*3600000,"chosen":true,"lines":[{"role":"user","text":"make the bass wider"},{"role":"assistant","text":"Widened it to 140%."}],"changes":[{"id":"old001:c4","family":"mixer","title":"Bass width 100% → 140%","state":"expired","note":"From an earlier session, so Kumi can't undo it now.","at":1}]}));
    h.has("Back to your conversation from 2 hours ago");
    h.has("Widened it to 140%.");
    assert!(h.screen().iter().any(|s| s.contains("Bass width") && s.contains("no undo")));
    h.close().await;
});
case!(memory_rows_forget_and_use, async {
    let c = Rc::new(Control::default());
    *c.memory.borrow_mut() = Some(serde_json::from_value(json!({"producer":[],"set":[],"saved":true})).unwrap());
    c.set("recipes", json!([]));
    c.set("techniques", json!([]));
    let h = Harness::with(120, 36, c, |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"technique","action":"kept","technique":{"id":"t1","name":"Neuro from a Reese","fits":"gritty neuro basses","source":"Au5 · Neuro bass"}}));
    h.has("◆ Kept a technique: Neuro from a Reese");
    let lines = h.screen();
    let now = lines.iter().position(|s| s.contains("NOW")).unwrap();
    assert!(lines[now + 1].contains("◆ Kept a technique: Neuro"));
    h.emit(json!({"type":"remembered","scope":"producer","note":{"id":"p1","text":"Prefers short reverbs","at":1}}));
    h.emit(json!({"type":"recipe","action":"saved","name":"Drum bus","steps":3}));
    h.has("✎ Noted about you: Prefers short reverbs");
    h.has("↻ Saved a recipe: Drum bus (3 steps)");
    let lines = h.screen();
    let row = strip(&lines);
    assert!(lines[row + 1].contains("↻ Drum bus") && lines[row + 1].contains("forget"));
    assert!(lines[row + 2].contains("✎ Prefers short reverbs"));
    assert!(lines[row + 3].contains("◆ Neuro from a Reese"));
    h.type_text(&click(&lines, row + 3, "forget")).await;
    assert!(h.calls().contains(&"forget-technique:t1".into()));
    h.emit(json!({"type":"technique","action":"forgot","technique":{"id":"t1","name":"Neuro from a Reese","fits":"gritty neuro basses"}}));
    assert!(h.screen()[row + 3].contains("forgotten"));
    h.has("◆ Forgot the technique: Neuro from a Reese");
    h.type_text(&click(&h.screen(), row + 2, "forget")).await;
    h.has("That was already gone.");
    // HISTORY's rows are kept between frames, and made again when what they show changes.
    assert!(h.screen()[row + 2].contains("forgotten"), "{}", h.screen()[row + 2]);
    h.emit(json!({"type":"technique","action":"used","technique":{"id":"t2","name":"Parallel drum crush","fits":"punchy drums"}}));
    h.has("◆ Using your technique: Parallel drum crush");
    h.close().await;
});
case!(a_forget_that_fails_says_so_and_can_be_tried_again, async {
    let c = Rc::new(Control::default());
    *c.memory.borrow_mut() = Some(serde_json::from_value(json!({"producer":[],"set":[],"saved":true})).unwrap());
    c.set("recipes", json!([]));
    c.set("forget-fails", json!("recipes.json is in use by another program"));
    let h = Harness::with(120, 36, c, |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"recipe","action":"saved","name":"Drum bus","steps":3}));
    h.has("↻ Saved a recipe: Drum bus (3 steps)");
    let lines = h.screen();
    let row = strip(&lines) + 1;
    assert!(lines[row].contains("↻ Drum bus") && lines[row].contains("forget"));
    h.type_text(&click(&lines, row, "forget")).await;
    assert!(h.calls().contains(&"forget-recipe:Drum bus".into()));
    h.has("recipes.json is in use by another program");
    assert!(!has(&h.screen(), "That was already gone."));
    // Still Kumi's to forget: the row offers it again.
    assert!(h.screen()[row].contains("forget") && !h.screen()[row].contains("forgotten"), "{}", h.screen()[row]);
    h.close().await;
});
case!(a_technique_offered_after_an_answer_is_answered_by_number, async {
    let c = Rc::new(Control::default());
    c.set("techniques", json!([]));
    let h = Harness::with(120, 36, c, |_| {});
    h.start().await;
    h.connect();
    let built = |h: &Harness, text: &str| {
        h.emit(json!({"type":"state","state":"running"}));
        h.emit(json!({"type":"text","text":text}));
        h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
        h.emit(json!({"type":"technique","action":"offered","technique":{"id":"","name":"Liquid bubbles","fits":"bubbly FM effects"}}));
        h.emit(json!({"type":"state","state":"idle"}));
    };
    let answers = |h: &Harness| h.calls().into_iter().filter(|c| c.starts_with("answer-technique:")).collect::<Vec<_>>();
    h.type_text("recreate the bubbles from this tutorial\r").await;
    built(&h, "Built the bubble chain on Bubbles.");
    h.has("◆ Keep this as a technique? Liquid bubbles");
    h.has("Keep “Liquid bubbles” as a technique?");
    h.has("1. Yes, keep it");
    h.type_text("1").await;
    assert!(answers(&h).is_empty(), "a number picks; enter answers");
    h.type_text("\r").await;
    h.wait_for_call("answer-technique:yes").await;
    let picks = |h: &Harness| h.calls().into_iter().filter(|c| c.starts_with("picked:")).collect::<Vec<_>>();
    assert_eq!(picks(&h), ["picked:technique:Liquid bubbles:yes"], "a number is the producer's answer");
    assert!(!has(&h.screen(), "Keep “Liquid bubbles” as a technique?"), "answering closes the offer");
    // Enter alone, or Esc, is a no.
    for (key, why) in [("\r", "a bare enter"), ("\x1b", "esc")] {
        h.type_text("another one\r").await;
        built(&h, "Built another.");
        h.has("Keep “Liquid bubbles” as a technique?");
        let before = answers(&h).len();
        h.type_text(key).await;
        h.wait_for_calls("answer-technique:", before + 1).await;
        assert_eq!(answers(&h).last().map(String::as_str), Some("answer-technique:no"), "{why}");
        assert!(!has(&h.screen(), "Keep “Liquid bubbles” as a technique?"), "{why}");
        assert_eq!(picks(&h).len(), 1, "{why} is the default, not a pick");
    }
    // Typing on goes to the input box and answers nothing.
    let answered = answers(&h).len();
    h.type_text("now a wetter one\r").await;
    built(&h, "Built it again, wetter.");
    h.has("Keep “Liquid bubbles” as a technique?");
    h.type_text("make it wetter").await;
    assert!(!has(&h.screen(), "Keep “Liquid bubbles” as a technique?"));
    h.has("make it wetter");
    assert_eq!(answers(&h).len(), answered);
    h.type_text("\x03").await;
    // An answer that ends on its own question keeps its options, and the offer doesn't cover them.
    h.type_text("add a reverb\r").await;
    built(&h, "Which track should get the reverb?\n\n1. Bubbles\n2. Drums");
    h.has("Your answer");
    assert!(!has(&h.screen(), "Keep “Liquid bubbles” as a technique?"));
    // So does a plain question; closing the answer's options answers nothing about the technique.
    h.type_text("\x1b").await;
    h.type_text("make it darker\r").await;
    built(&h, "Darker now. Should the riser come in earlier?");
    assert!(!has(&h.screen(), "Keep “Liquid bubbles” as a technique?"));
    assert!(!has(&h.screen(), "Your answer"));
    h.has("◆ Keep this as a technique? Liquid bubbles · say “keep the technique”");
    assert_eq!(answers(&h).len(), answered);
    h.close().await;
});
case!(stop_live_and_held_cancel_refusal, async {
    let c = Rc::new(Control::default());
    c.stop.set(true);
    let h = Harness::with(120, 36, c.clone(), |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"state","state":"running"}));
    h.type_text("/stop\r").await;
    h.has("■ Stopped");
    assert!(h.calls().contains(&"cancel".into()));
    h.emit(json!({"type":"state","state":"idle"}));
    c.set("stop-result", json!(false));
    h.type_text("/stop\r").await;
    h.has("press space in Live");
    h.emit(json!({"type":"connection","state":"disconnected"}));
    h.type_text("/stop\r").await;
    assert_eq!(h.calls().iter().filter(|s| s.as_str() == "stop-live").count(), 2);
    h.close().await;
    let c = Rc::new(Control::default());
    c.set("cancel-idle", json!(true));
    let h = Harness::with(120, 36, c.clone(), |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"state","state":"running"}));
    h.type_text("make the bass louder\r").await;
    h.has("↳ make the bass louder");
    h.type_text("\x1b").await;
    h.wait_for_call("cancel").await;
    assert!(h.calls().contains(&"cancel".into()));
    assert!(!h.calls().iter().any(|s| s.starts_with("submit:")));
    h.has("make the bass louder");
    assert!(!has(&h.screen(), "↳ make the bass louder"));
    h.type_text("\x15").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.type_text("too long, say\r").await;
    c.set("submit-error", json!("Enter a nonempty prompt of at most 16 KiB"));
    h.emit(json!({"type":"state","state":"idle"}));
    delay(10).await;
    h.has("at most 16 KiB");
    h.has("too long, say");
    assert!(!has(&h.screen(), "↳ too long, say"));
    h.close().await;
});
case!(reconnect_refused_undo_and_narrow_undo, async {
    let c = Rc::new(Control::default());
    c.set("reconnect", json!(true));
    let h = Harness::with(120, 36, c, |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"change","change":{"id":"c5","family":"tempo","title":"Tempo 120 → 126 BPM","state":"applied","at":1}}));
    h.type_text("/reconnect\r").await;
    assert!(h.calls().contains(&"reconnect".into()));
    assert!(h.screen().iter().any(|s| s.contains("Tempo 120 → 126 BPM") && s.contains("no undo")));
    let change = json!({"id":"c7","family":"rename","title":"Renamed track “Bass” → “Sub”","state":"applied","at":1});
    h.emit(json!({"type":"change","change":change}));
    let mut kept = change;
    kept["state"] = json!("kept");
    kept["note"] = json!("It changed in Live since, so Kumi left it as it is.");
    *h.control.undo.borrow_mut() = Some(serde_json::from_value(kept).unwrap());
    h.type_text("/undo\r").await;
    assert!(h.calls().contains(&"undo:last".into()));
    h.has("Kept: Renamed track “Bass” → “Sub”.");
    h.has("It changed in Live since");
    assert!(h.screen().iter().any(|s| s.contains("Renamed track “Bass” → “Sub”") && s.contains("kept")));
    h.close().await;
    let h = Harness::new(80, 24);
    h.start().await;
    h.connect();
    let change = json!({"id":"c3","family":"tempo","title":"Tempo 120 → 130 BPM","state":"applied","at":1});
    h.emit(json!({"type":"change","change":change}));
    let mut undone = change;
    undone["state"] = json!("undone");
    *h.control.undo.borrow_mut() = Some(serde_json::from_value(undone).unwrap());
    let lines = h.screen();
    let row = lines.iter().position(|s| s.contains("Tempo 120 → 130 BPM") && s.contains("undo")).unwrap();
    h.type_text(&click(&lines, row, "undo")).await;
    assert!(h.calls().contains(&"undo:c3".into()));
    h.close().await;
});
case!(after_reconnecting_history_says_kumi_cant_undo_what_it_could, async {
    let c = Rc::new(Control::default());
    c.set("reconnect", json!(true));
    let h = Harness::with(120, 36, c, |_| {});
    h.start().await;
    h.connect();
    h.emit(json!({"type":"change","change":{"id":"c8","family":"tempo","title":"Tempo 120 → 128 BPM","state":"applied","at":1}}));
    let lines = h.screen();
    let history = lines.iter().position(|s| s.contains("HISTORY")).unwrap();
    let row = lines[history + 1].clone();
    assert!(row.contains("Tempo 120 → 128 BPM") && row.trim_end().ends_with("undo") && !row.contains("no undo"), "{row}");
    // Reconnecting changes the records in place: HISTORY's own row says so, not only NOW.
    h.type_text("/reconnect\r").await;
    let row = h.screen()[history + 1].clone();
    assert!(row.contains("Tempo 120 → 128 BPM") && row.contains("no undo"), "{row}");
    h.close().await;
});
case!(a_picker_over_the_dock_takes_the_clicks_on_what_it_hides, async {
    let h = Harness::new(80, 24);
    h.start().await;
    h.connect();
    h.emit(json!({"type":"change","change":{"id":"c4","family":"tempo","title":"Tempo 120 → 128 BPM","state":"applied","at":1}}));
    let docked = h.screen();
    let row = docked.iter().position(|s| s.contains("Tempo 120 → 128 BPM") && s.contains("undo")).unwrap();
    // The answers picker opens over the dock: its lower right is where undo was.
    h.type_text("which one?\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Which reverb?\n\n1. Hall\n2. Plate\n3. Spring\n4. Room\n5. None"}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.has("Your answer");
    assert!(!h.screen()[row].contains("undo"), "the picker covers the dock's undo:\n{}", h.screen().join("\n"));
    h.type_text(&click(&docked, row, "undo")).await;
    assert!(!h.calls().iter().any(|c| c.starts_with("undo:")), "a click on the picker undid a change: {:?}", h.calls());
    h.close().await;
});
case!(history_scroll_mouse_keyboard_and_badges, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    for i in 1..=40 {
        h.emit(json!({"type":"change","change":{"id":format!("c{i}"),"family":"tempo","title":format!("Tempo change {i}"),"state":"applied","at":i}}));
    }
    let lines = h.screen();
    let row = strip(&lines);
    assert!(lines[row].contains("HISTORY 40"));
    assert!(lines[row + 1].contains("Tempo change 40"));
    assert!(regex::Regex::new("↓ [0-9]+ more").unwrap().is_match(&lines.join("\n")));
    let x = lines[row].find("HISTORY").unwrap() + 2;
    h.type_text(&format!("\x1b[<65;{x};{}M", row + 4)).await;
    let lines = h.screen();
    assert!(lines[row + 1].contains("↑ 4 more"));
    assert!(lines[row + 2].contains("Tempo change 36"));
    h.emit(json!({"type":"change","change":{"id":"c41","family":"tempo","title":"Tempo change 41","state":"applied","at":41}}));
    assert!(h.screen()[row + 2].contains("Tempo change 36"));
    *h.control.undo.borrow_mut() =
        Some(serde_json::from_value(json!({"id":"c36","family":"tempo","title":"Tempo change 36","state":"undone","at":36})).unwrap());
    h.type_text(&click(&h.screen(), row + 2, "undo")).await;
    assert!(h.calls().contains(&"undo:c36".into()));
    *h.control.undo.borrow_mut() =
        Some(serde_json::from_value(json!({"id":"c35","family":"tempo","title":"Tempo change 35","state":"undone","at":35})).unwrap());
    h.type_text("\x1b[Z").await;
    h.type_text("\x1b[B").await;
    h.type_text("\r").await;
    assert!(h.calls().contains(&"undo:c35".into()));
    assert!(h.has_selection_highlight());
    h.type_text("\x1b").await;
    h.wait_until_selection_clears().await;
    h.type_text("x").await;
    h.has("x");
    h.close().await;
    let c = Rc::new(Control::default());
    *c.tree.borrow_mut() = Some(
        serde_json::from_value(
            json!({"trackRef":"3:track:3","devices":[{"ref":"a","name":"Saturator","className":"Saturator","deviceType":"audio_effect"}]}),
        )
        .unwrap(),
    );
    let h = Harness::with(120, 36, c, |o| o.icons = Some(kumi::tui::icons::IconStyle::Badges));
    h.start().await;
    h.connect();
    h.emit(json!({"type":"focus","focus":{"track":{"name":"4-Audio","kind":"audio"},"trackRef":"3:track:3","device":"Saturator","detail":"Device"}}));
    delay(5).await;
    h.has("AT 4-Audio");
    h.has("└ FX Saturator");
    h.close().await;
});
fn willington(on: bool) -> (kumi::willington::WillingtonControl, Rc<RefCell<Vec<bool>>>) {
    let on = Rc::new(Cell::new(on));
    let switched = Rc::new(RefCell::new(Vec::new()));
    let control = kumi::willington::WillingtonControl {
        on: {
            let on = on.clone();
            Rc::new(move || Some(on.get()))
        },
        set: {
            let switched = switched.clone();
            Rc::new(move |value| {
                on.set(value);
                switched.borrow_mut().push(value);
                let said = if value { kumi::willington::TURNED_ON } else { kumi::willington::TURNED_OFF };
                async move { Ok(said) }.boxed_local()
            })
        },
    };
    (control, switched)
}
case!(willington_off_is_said_at_the_start_and_its_command_switches_it, async {
    let (control, switched) = willington(false);
    let c = Rc::new(Control::default());
    let h = Harness::with(160, 40, c.clone(), move |o| o.willington = Some(control));
    h.start().await;
    // On the welcome screen, which stays.
    h.has("Willington bindings are OFF currently, type /willington to toggle them on");
    h.has("Ask anything about production.");
    h.type_text("/will").await;
    h.has("Willington's bindings in Live");
    h.type_text("ington\r").await;
    h.has("Willington bindings are ON: Kumi can map rack macros");
    assert!(!has(&h.screen(), "Willington bindings are OFF currently"));
    h.type_text("/willington\r").await;
    h.has("Willington bindings are OFF. /willington turns them on again.");
    assert_eq!(*switched.borrow(), [true, false]);
    assert_eq!(c.calls.borrow().iter().filter(|call| *call == "reconfigure").count(), 2);
    h.close().await;
    // A conversation carried on at the start replaces the welcome screen: it's said below that instead.
    let (control, _) = willington(false);
    let h = Harness::with(160, 40, Rc::new(Control::default()), move |o| o.willington = Some(control));
    h.start().await;
    h.emit(
        json!({"type":"resumed","savedAt":kumi_common::time::now_ms()-2*3600000,"lines":[{"role":"user","text":"make the bass wider"}]}),
    );
    h.has("Willington bindings are OFF currently, type /willington to toggle them on");
    h.close().await;
    // On, or without Willington in the bridge: nothing at the start, and no command without it.
    let (control, _) = willington(true);
    let h = Harness::with(160, 40, Rc::new(Control::default()), move |o| o.willington = Some(control));
    h.start().await;
    assert!(!has(&h.screen(), "Willington bindings"));
    h.close().await;
    let h = Harness::with(160, 40, Rc::new(Control::default()), |_| {});
    h.start().await;
    h.type_text("/willington\r").await;
    h.has("There's no /willington command.");
    h.close().await;
});
case!(fast_turns_on_the_tier_the_model_offers_and_shows_it_beside_the_model, async {
    let m = FakeModels::catalog();
    let h = model_harness(m.clone(), 120);
    h.start().await;
    h.type_text("/fast\r").await;
    h.has("Fast is on: 2x speed, increased usage. /fast again turns it off.");
    h.has("GPT-6 Astra · fast");
    h.type_text("/fast\r").await;
    h.has("Back to the standard tier.");
    assert!(!has(&h.screen(), "Astra · fast"));
    h.type_text("/fast\r").await;
    *m.model.borrow_mut() = Some("openai-codex/gpt-6-luna".into());
    h.type_text("/fast\r").await;
    h.has("Back to the standard tier.");
    h.type_text("/fast\r").await;
    h.has("GPT-6 Luna has no faster tier to turn on.");
    assert_eq!(
        m.calls.borrow().iter().filter(|c| c.starts_with("fast:")).collect::<Vec<_>>(),
        ["fast:true", "fast:false", "fast:true", "fast:false"],
        "on a model without a tier, a saved \"on\" still turns off"
    );
    // Choosing a model that Fast applies to says so, since it costs more.
    *m.model.borrow_mut() = Some("openai-codex/gpt-6-astra".into());
    h.type_text("/fast\r").await;
    *m.model.borrow_mut() = Some("openai-codex/gpt-6-luna".into());
    h.type_text("/model\r").await;
    h.type_text("astra").await;
    h.type_text("\r").await;
    h.has("Fast is on: 2x speed, increased usage; /fast turns it off.");
    h.close().await;
});
