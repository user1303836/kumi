//! A producer away from Kumi's window: the terminal's status as Kumi works, and a desktop notification and a
//! line in the DAW's window only while Kumi's window is behind another.
#[path = "../support/models.rs"]
mod models;
#[path = "../support/tui_app.rs"]
mod support;
use base64::Engine;
use kumi::tui::{
    alerts::{NotifyChannel, TerminalProfile},
    app::AwaySettings,
    tty::RESTORE,
};
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

const FOCUS_OUT: &str = "\x1b[O";
const FOCUS_IN: &str = "\x1b[I";

fn profile(notify: NotifyChannel) -> TerminalProfile {
    TerminalProfile { status: true, notify, ..TerminalProfile::quiet() }
}

fn away(notify: NotifyChannel) -> Harness {
    away_with(Rc::new(Control::default()), notify, AwaySettings::default())
}

fn away_with(control: Rc<Control>, notify: NotifyChannel, settings: AwaySettings) -> Harness {
    Harness::with(120, 36, control, |o| {
        o.terminal = Some(profile(notify));
        o.away = settings;
        o.alert_gap_ms = Some(60.);
    })
}

/// The producer asks, and Kumi answers in `elapsed_ms`.
async fn answer(h: &Harness, text: &str, elapsed_ms: u64) {
    h.type_text("carry on\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":text}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":elapsed_ms}));
    h.emit(json!({"type":"state","state":"idle"}));
}

/// Each OSC 7501 report written so far: its state and its words (decoded).
fn reports(h: &Harness) -> Vec<(String, String)> {
    h.screen();
    let written = h.output.text.borrow();
    written
        .split("\x1b]7501;")
        .skip(1)
        .map(|rest| {
            let body = rest.split("\x1b\\").next().unwrap();
            let get = |key: &str| body.split(':').find_map(|pair| pair.strip_prefix(&format!("{key}="))).unwrap_or("").to_string();
            let words =
                base64::engine::general_purpose::STANDARD.decode(get("msg")).map(|b| String::from_utf8(b).unwrap()).unwrap_or_default();
            let kind = get("kind");
            (if kind.is_empty() { get("state") } else { format!("{}:{kind}", get("state")) }, words)
        })
        .collect()
}

fn last_state(h: &Harness) -> String {
    reports(h).last().map(|(state, _)| state.clone()).unwrap_or_default()
}

/// The notifications written to the terminal (OSC 777's bodies).
fn notifications(h: &Harness) -> Vec<String> {
    h.screen();
    let written = h.output.text.borrow();
    written.split("\x1b]777;notify;Kumi;").skip(1).map(|rest| rest.split("\x1b\\").next().unwrap().to_string()).collect()
}

async fn until(what: &str, ready: impl Fn() -> bool) {
    for _ in 0..300 {
        if ready() {
            return;
        }
        delay(10).await;
    }
    panic!("waited for {what}");
}

case!(the_terminal_shows_what_kumi_is_doing_and_quitting_clears_it, async {
    let h = away(NotifyChannel::None);
    h.start().await;
    h.connect();
    h.screen();
    assert_eq!(last_state(&h), "idle");
    h.type_text("make the bass darker\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    assert_eq!(last_state(&h), "working");
    let title =
        h.output.text.borrow().rsplit("\x1b]7501;").next().unwrap().split(':').find_map(|p| p.strip_prefix("title=")).map(str::to_string);
    let title = base64::engine::general_purpose::STANDARD.decode(title.expect("the Set as its title")).unwrap();
    assert_eq!(String::from_utf8(title).unwrap(), "Night Drive");
    // A wait says why, without its countdown: the status changes when what Kumi does changes, not each second (new
    // words go at most once a second).
    h.emit(json!({"type":"retry","reason":"ChatGPT is busy (HTTP 429)","waitMs":5000}));
    until("the wait's words", || reports(&h).last().is_some_and(|(_, words)| words == "working · ChatGPT is busy (HTTP 429)")).await;
    let count = reports(&h).len();
    delay(1300).await;
    assert_eq!(reports(&h).len(), count, "nothing ticks");
    h.emit(json!({"type":"text","text":"## Made the bass darker\n\nThe cutoff is down 2 kHz."}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":4000}));
    h.emit(json!({"type":"state","state":"idle"}));
    assert_eq!(reports(&h).last().unwrap(), &("done".to_string(), "Made the bass darker".to_string()));
    // A question waits on the producer.
    answer(&h, "Which pad?\n\n1. Warm\n2. Glassy", 900).await;
    assert_eq!(last_state(&h), "blocked:question");
    h.type_text("1\r").await;
    h.wait_for_call("submit:Warm").await;
    // A failed answer says so; Esc leaves Kumi idle.
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(
        json!({"type":"error","message":"ChatGPT is having trouble (HTTP 500); try again.","kind":"provider","provider":"openai-codex"}),
    );
    h.emit(json!({"type":"state","state":"idle"}));
    assert_eq!(reports(&h).last().unwrap(), &("error".to_string(), "ChatGPT is having trouble (HTTP 500); try again.".to_string()));
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"cancelled"},"elapsedMs":300}));
    h.emit(json!({"type":"state","state":"idle"}));
    assert_eq!(last_state(&h), "idle");
    // Nothing new to say, nothing written: the same status isn't sent twice.
    let count = reports(&h).len();
    h.emit(json!({"type":"notice","message":"Hello"}));
    assert_eq!(reports(&h).len(), count);
    h.close().await;
    assert!(h.output.text.borrow().contains(&format!("\x1b]7501;state=clear\x1b\\{RESTORE}")), "quitting clears Kumi's status");
});

case!(a_notification_and_the_daws_line_only_while_kumis_window_is_behind_another, async {
    let h = away(NotifyChannel::Osc777);
    h.start().await;
    h.connect();
    // In front (no focus report yet means it is): nothing.
    answer(&h, "Made the bass darker.", 20_000).await;
    delay(20).await;
    assert!(notifications(&h).is_empty());
    assert!(!h.calls().iter().any(|c| c.starts_with("daw-message:")));
    h.type_text(FOCUS_OUT).await;
    answer(&h, "Made the bass darker.", 20_000).await;
    until("the notification", || notifications(&h).len() == 1).await;
    assert_eq!(notifications(&h), ["Done: Made the bass darker."]);
    h.wait_for_call("daw-message:Kumi · done · Made the bass darker.").await;
    // A quick answer isn't worth one; a question always is.
    delay(80).await;
    answer(&h, "Renamed it.", 900).await;
    delay(80).await;
    assert_eq!(notifications(&h).len(), 1);
    answer(&h, "Which pad?\n\n1. Warm\n2. Glassy", 900).await;
    until("the question's notification", || notifications(&h).len() == 2).await;
    assert_eq!(notifications(&h)[1], "Needs you: pick 1–2");
    h.wait_for_call("daw-message:Kumi needs you · pick 1–2 in Kumi").await;
    // Back at the window, the done answer has been seen, and nothing more is told.
    h.type_text("\x1b").await;
    h.type_text(FOCUS_IN).await;
    delay(80).await;
    h.emit(json!({"type":"error","message":"Something broke."}));
    delay(80).await;
    assert_eq!(notifications(&h).len(), 2);
    h.close().await;
});

case!(alerts_keep_a_gap_and_the_newest_of_those_waiting_goes, async {
    let h = away(NotifyChannel::Osc777);
    h.start().await;
    h.connect();
    h.type_text(FOCUS_OUT).await;
    h.emit(json!({"type":"error","message":"First."}));
    h.emit(json!({"type":"error","message":"Second."}));
    h.emit(json!({"type":"error","message":"Third."}));
    until("two notifications", || notifications(&h).len() >= 2).await;
    delay(100).await;
    assert_eq!(notifications(&h), ["Stopped: First.", "Stopped: Third."]);
    // Coming back before a waiting one goes drops it.
    h.emit(json!({"type":"error","message":"Fourth."}));
    h.emit(json!({"type":"error","message":"Fifth."}));
    h.type_text(FOCUS_IN).await;
    delay(150).await;
    assert_eq!(notifications(&h).len(), 2);
    h.close().await;
});

case!(no_notification_over_a_take_being_recorded_until_it_stops, async {
    let control = Rc::new(Control::default());
    *control.extra.borrow_mut() = json!({"recording": true});
    let h = away_with(control.clone(), NotifyChannel::AppleScript, AwaySettings::default());
    h.start().await;
    h.connect();
    h.type_text(FOCUS_OUT).await;
    answer(&h, "Bounced the stems.", 30_000).await;
    // The DAW's own line is silent: it shows at once.
    h.wait_for_call("daw-message:Kumi · done · Bounced the stems.").await;
    delay(150).await;
    assert!(h.notified.borrow().is_empty(), "nothing over the take");
    control.extra.borrow_mut()["recording"] = json!(false);
    until("the notification once recording stopped", || !h.notified.borrow().is_empty()).await;
    let (program, args) = h.notified.borrow()[0].clone();
    assert_eq!(program, "osascript");
    assert_eq!(args, ["-e", "display notification \"Done: Bounced the stems.\" with title \"Kumi\""]);
    h.close().await;
});

case!(settings_turn_each_off_and_the_status_stays, async {
    let quiet = AwaySettings { notifications: false, live_status_bar: false, ..AwaySettings::default() };
    let h = away_with(Rc::new(Control::default()), NotifyChannel::Osc777, quiet);
    h.start().await;
    h.connect();
    h.type_text(FOCUS_OUT).await;
    answer(&h, "Made the bass darker.", 20_000).await;
    delay(100).await;
    assert!(notifications(&h).is_empty());
    assert!(!h.calls().iter().any(|c| c.starts_with("daw-message:")));
    assert_eq!(last_state(&h), "done", "the terminal's status is passive: it stays on");
    h.close().await;
    // Only the DAW's line, without notifications.
    let line = AwaySettings { notifications: false, ..AwaySettings::default() };
    let h = away_with(Rc::new(Control::default()), NotifyChannel::Osc777, line);
    h.start().await;
    h.connect();
    h.type_text(FOCUS_OUT).await;
    answer(&h, "Made the bass darker.", 20_000).await;
    h.wait_for_call("daw-message:Kumi · done · Made the bass darker.").await;
    assert!(notifications(&h).is_empty());
    h.close().await;
});

case!(a_goals_end_is_told_once_in_place_of_its_answers_done, async {
    let h = away(NotifyChannel::Osc777);
    h.start().await;
    h.connect();
    h.type_text(FOCUS_OUT).await;
    let goal = |state: &str, verdict: Option<&str>| {
        let mut status = json!({"type":"objective","objective":"Make the mix louder","state":state,"turns":3,"turnBudget":12,"elapsedMs":60000,"budgetMs":3600000,"measured":true});
        if let Some(verdict) = verdict {
            status["verdict"] = json!(verdict);
        }
        h.emit(status);
    };
    h.emit(json!({"type":"state","state":"running"}));
    goal("running", None);
    let (_, words) = reports(&h).last().unwrap().clone();
    assert!(words.starts_with("goal · turn 4/12"), "{words}");
    assert!(!words.contains(':'), "no clock in the status: {words}");
    goal("done", Some("complete"));
    h.emit(json!({"type":"text","text":"Louder by 2 LU."}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":600000}));
    h.emit(json!({"type":"state","state":"idle"}));
    until("the goal's notification", || !notifications(&h).is_empty()).await;
    delay(150).await;
    assert_eq!(notifications(&h), ["Goal met: Make the mix louder"]);
    h.close().await;
});

// A loop's progress is its rounds against its budget's (#625), in Kumi's status and the terminal's bar: three judged with
// nine left is a quarter of the way.
case!(a_loops_progress_is_its_rounds_against_its_budget, async {
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| {
        o.terminal = Some(TerminalProfile { progress: true, ..profile(NotifyChannel::None) });
        o.alert_gap_ms = Some(60.);
    });
    h.start().await;
    h.connect();
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"loop","state":"running","request":"master this to -9 LUFS","rounds":3,"kept":2,"reverted":1,"listens":4,"elapsedMs":60000,"roundsLeft":9}));
    until("the loop's progress", || {
        reports(&h);
        h.output.text.borrow().contains("\x1b]9;4;1;25")
    })
    .await;
    assert!(h.output.text.borrow().contains(":progress=25:"), "{:?}", h.output.text.borrow());
    h.close().await;
});

case!(a_terminal_that_never_says_where_focus_is_asks_the_system_only_when_theres_something_to_tell, async {
    let in_front = Rc::new(std::cell::Cell::new(Some(false)));
    let asked = Rc::new(std::cell::Cell::new(0));
    let (front, count) = (in_front.clone(), asked.clone());
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| {
        o.terminal = Some(profile(NotifyChannel::Osc777));
        o.alert_gap_ms = Some(60.);
        o.front_app = Some(Rc::new(move || {
            count.set(count.get() + 1);
            let answer = front.get();
            Box::pin(async move { answer })
        }));
    });
    h.start().await;
    h.connect();
    delay(50).await;
    assert_eq!(asked.get(), 0, "never on a timer");
    answer(&h, "Made the bass darker.", 20_000).await;
    until("the notification", || notifications(&h).len() == 1).await;
    assert_eq!(asked.get(), 1);
    // Kumi's terminal is in front: nothing; and nothing when the system can't say.
    for answer_now in [Some(true), None] {
        in_front.set(answer_now);
        delay(80).await;
        answer(&h, "Made it brighter.", 20_000).await;
        delay(80).await;
        assert_eq!(notifications(&h).len(), 1, "{answer_now:?}");
    }
    // Once the terminal says where focus is, its word counts and the system isn't asked.
    let before = asked.get();
    h.type_text(FOCUS_OUT).await;
    in_front.set(Some(true));
    delay(80).await;
    answer(&h, "Made it wider.", 20_000).await;
    until("the second notification", || notifications(&h).len() == 2).await;
    assert_eq!(asked.get(), before);
    h.close().await;
});

fn with_models(width: i32, settings: AwaySettings) -> (Harness, Rc<models::FakeModels>) {
    with_models_on(Rc::new(Control::default()), width, settings)
}

fn with_models_on(control: Rc<Control>, width: i32, settings: AwaySettings) -> (Harness, Rc<models::FakeModels>) {
    let fake = models::FakeModels::catalog();
    fake.signed_in.borrow_mut().insert(kumi_ai::providers::ProviderId::parse("anthropic").unwrap());
    let m = fake.clone();
    *control.models.borrow_mut() = Some(m);
    let h = Harness::with(width, 36, control, move |o| {
        o.terminal = Some(profile(NotifyChannel::Osc777));
        o.away = settings;
        o.alert_gap_ms = Some(60.);
    });
    (h, fake)
}

case!(a_plans_limit_shows_when_kumi_carries_on_and_the_fallback_is_named_while_it_answers, async {
    let control = Rc::new(Control::default());
    let (h, _) = with_models_on(control.clone(), 140, AwaySettings::default());
    h.start().await;
    h.connect();
    h.type_text(FOCUS_OUT).await;
    h.type_text("carry on\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    let two_hours: u64 = 2 * 3600 * 1000;
    h.emit(json!({"type":"retry","reason":"ChatGPT's plan limit","waitMs":two_hours,"wait":"limit"}));
    h.has("carrying on at");
    h.has("(in 2h) · ChatGPT's");
    // Told once, as a wait.
    until("the wait's notification", || notifications(&h).len() == 1).await;
    assert!(notifications(&h)[0].starts_with("Waiting: ChatGPT's plan limit · carrying on at "), "{:?}", notifications(&h));
    let (state, words) = reports(&h).last().unwrap().clone();
    assert_eq!(state, "working");
    assert!(words.contains("ChatGPT's plan limit · carrying on at") && !words.contains("(in"), "steady words: {words}");
    // Offline: tries again, and says so.
    h.emit(json!({"type":"retry","reason":"ChatGPT couldn't be reached","waitMs":20000,"wait":"offline"}));
    h.has("offline · trying again in 20s");
    until("the status words", || reports(&h).last().is_some_and(|(_, words)| words.contains("offline · ChatGPT couldn't be reached")))
        .await;
    // A certificate or a proxy in the way: the same wait, named.
    h.emit(json!({"type":"retry","reason":"ChatGPT couldn't be reached: the secure connection didn't check out (TLS)","waitMs":20000,"wait":"offline","cause":"tls"}));
    h.has("TLS failed · trying again in 20s");
    until("the status words", || {
        reports(&h).last().is_some_and(|(_, words)| {
            words.contains("ChatGPT couldn't be reached: the secure connection didn't check out (TLS)") && !words.contains("offline")
        })
    })
    .await;
    h.emit(json!({"type":"retry","reason":"ChatGPT couldn't be reached: the proxy didn't let Kumi through","waitMs":20000,"wait":"offline","cause":"proxy"}));
    h.has("proxy refused · trying again in 20s");
    // One that isn't answering says so, as the words do.
    h.emit(json!({"type":"retry","reason":"ChatGPT couldn't be reached: the proxy isn't answering","waitMs":20000,"wait":"offline","cause":"proxy","proxyDown":true}));
    h.has("proxy not answering · trying again in 20s");
    // The fallback takes over: said once in the conversation, and named in the header while the session says it answers.
    control.extra.borrow_mut()["answering"] = json!("anthropic/claude-sonnet-5-5");
    h.emit(json!({"type":"model-switch","model":"anthropic/claude-sonnet-5-5","reason":"ChatGPT's plan limit resets at 14:20"}));
    h.has("ChatGPT's plan limit resets at 14:20; carrying on with claude-sonnet-5-5");
    h.has("claude-sonnet-5-5 · fallback");
    h.emit(json!({"type":"text","text":"Louder now."}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":3000}));
    h.emit(json!({"type":"state","state":"idle"}));
    control.extra.borrow_mut().as_object_mut().unwrap().remove("answering");
    h.emit(json!({"type":"model-switch","model":"openai-codex/gpt-6-astra","reason":"ChatGPT is answering again","back":true}));
    h.has("ChatGPT is answering again: back to gpt-6-astra.");
    h.wait_until_hidden("· fallback").await;
    h.close().await;
});

case!(near_a_plans_limit_the_header_says_how_much_is_used_and_when_it_resets, async {
    let (h, _) = with_models(140, AwaySettings::default());
    kumi_ai::providers::usage::forget();
    h.start().await;
    h.connect();
    let at = (kumi_common::time::now_ms_f64() / 1000. + 3600.) as i64;
    let headers = |used: &str| -> std::collections::BTreeMap<String, String> {
        [("x-codex-primary-used-percent".to_string(), used.to_string()), ("x-codex-primary-reset-at".to_string(), at.to_string())]
            .into_iter()
            .collect()
    };
    kumi_ai::providers::usage::record("openai-codex", &headers("62"));
    h.emit(json!({"type":"notice","message":"redraw"}));
    assert!(!h.screen()[0].contains("plan "), "below 80% it says nothing: {}", h.screen()[0]);
    kumi_ai::providers::usage::record("openai-codex", &headers("84"));
    h.emit(json!({"type":"notice","message":"redraw again"}));
    h.has("plan 84% · resets");
    kumi_ai::providers::usage::forget();
    h.close().await;
});

case!(a_limit_too_far_off_offers_a_fallback_that_carries_on_at_once, async {
    let (h, fake) = with_models(140, AwaySettings::default());
    h.start().await;
    h.connect();
    h.type_text("make it louder\r").await;
    h.wait_for_call("submit:make it louder").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"error","message":"ChatGPT's plan limit is reached; it resets at Fri 09:00.","kind":"usage-limit","provider":"openai-codex"}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.has("Carry on without ChatGPT?");
    h.has("Choose a fallback model");
    h.has("Sign in to another provider");
    h.type_text("\r").await;
    h.has("Choose a fallback model");
    h.type_text("sonnet").await;
    h.has("Claude Sonnet 5.5");
    h.type_text("\r").await;
    until("the fallback", || fake.calls.borrow().iter().any(|c| c == "fallback:anthropic/claude-sonnet-5-5")).await;
    // The message that stopped goes again, and the fallback carries on with it.
    h.wait_for_calls("submit:make it louder", 2).await;
    h.has("claude-sonnet-5-5 carries on from your next message");
    h.close().await;
});

case!(ctrl_f_in_the_model_list_sets_the_fallback_and_again_clears_it, async {
    let (h, fake) = with_models(140, AwaySettings::default());
    h.start().await;
    h.connect();
    h.type_text("/model\r").await;
    h.has("ctrl+f sets the fallback");
    h.type_text("sonnet").await;
    h.type_text("\x06").await;
    until("the fallback set", || fake.calls.borrow().iter().any(|c| c == "fallback:anthropic/claude-sonnet-5-5")).await;
    h.has("carries on from your next message whenever");
    h.type_text("/model\r").await;
    h.has("fallback");
    h.type_text("sonnet").await;
    h.type_text("\x06").await;
    until("the fallback cleared", || fake.calls.borrow().iter().any(|c| c == "fallback:")).await;
    h.has("No fallback");
    h.close().await;
});

async fn ask_with_a_recommendation(h: &Harness) {
    h.type_text("carry on\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"objective","objective":"Make the mix louder","state":"running","turns":1,"turnBudget":12,"elapsedMs":1000,"budgetMs":3600000,"measured":true}));
    h.emit(json!({"type":"text","text":"Which bus should get the limiter?\n\n1. Master\n2. Drums bus (recommended)\n3. Neither"}));
    h.emit(json!({"type":"objective","objective":"Make the mix louder","state":"paused","turns":2,"turnBudget":12,"elapsedMs":2000,"budgetMs":3600000,"verdict":"blocked","reason":"Needs a choice","measured":true}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":2000}));
    h.emit(json!({"type":"state","state":"idle"}));
}

case!(nobody_there_kumi_takes_its_recommended_answer_after_the_countdown_and_any_key_stops_it, async {
    // Each case waits out a countdown of a second or more, in a Kumi of its own: all at once.
    let quick = AwaySettings { answer_after: 1, ..AwaySettings::default() };
    let taken = async {
        let h = away_with(Rc::new(Control::default()), NotifyChannel::None, quick.clone());
        h.start().await;
        h.connect();
        ask_with_a_recommendation(&h).await;
        h.wait_for("Taking 2 (recommended) in 1 s · any key cancels").await;
        h.has("2. Drums bus");
        h.has("recommended");
        h.wait_for_call("submit:Drums bus").await;
        h.has("No answer in 1 s; went with 2.");
        assert!(!h.calls().iter().any(|c| c.starts_with("picked:")), "not the producer's pick: {:?}", h.calls());
        h.close().await;
    };
    // Where /goal is offered, the goal that stopped on the question carries on with the answer.
    let goal = async {
        let control = Rc::new(Control::default());
        *control.extra.borrow_mut() = json!({"goal": true});
        let h = away_with(control, NotifyChannel::None, quick.clone());
        h.start().await;
        h.connect();
        ask_with_a_recommendation(&h).await;
        h.wait_for_call("goal:answer: Drums bus").await;
        assert!(!h.calls().iter().any(|c| c == "submit:Drums bus"), "not a message of its own: {:?}", h.calls());
        h.close().await;
    };
    // A key: the producer's there.
    let key = async {
        let h = away_with(Rc::new(Control::default()), NotifyChannel::None, quick.clone());
        h.start().await;
        h.connect();
        ask_with_a_recommendation(&h).await;
        h.wait_for("any key cancels").await;
        h.type_text("\x1b[B").await;
        delay(1300).await;
        assert!(!h.calls().iter().any(|c| c == "submit:Drums bus"), "{:?}", h.calls());
        assert!(!has(&h.screen(), "any key cancels"));
        h.close().await;
    };
    // Someone at the window, with no run under way: no countdown; and 0 never counts down.
    let no_countdown = |settings: AwaySettings, run: bool| async move {
        let h = away_with(Rc::new(Control::default()), NotifyChannel::None, settings);
        h.start().await;
        h.connect();
        if run {
            ask_with_a_recommendation(&h).await;
        } else {
            answer(&h, "Which bus?\n\n1. Master\n2. Drums bus (recommended)", 900).await;
        }
        h.has("Your answer");
        delay(1300).await;
        assert!(!h.calls().iter().any(|c| c == "submit:Drums bus"), "{:?}", h.calls());
        h.close().await;
    };
    tokio::join!(
        taken,
        goal,
        key,
        no_countdown(quick.clone(), false),
        no_countdown(AwaySettings { answer_after: 0, ..AwaySettings::default() }, true)
    );
});

case!(a_think_leaves_the_status_steady_while_its_time_runs_and_its_headline_streams, async {
    let h = away(NotifyChannel::None);
    h.start().await;
    h.connect();
    h.type_text("make the kick punch through the bass\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"reasoning","text":"**Weighing the kick**"}));
    until("the think's words", || reports(&h).last().is_some_and(|(_, words)| words == "working · thinking")).await;
    let count = reports(&h).len();
    // A second goes by and the headline moves on: NOW says so, the status doesn't change.
    delay(1300).await;
    h.emit(json!({"type":"reasoning","text":"\n\n**Checking the sidechain**"}));
    h.has("Checking the sidechain");
    delay(1100).await;
    assert_eq!(reports(&h).len(), count, "nothing ticks: {:?}", reports(&h));
    h.close().await;
});
