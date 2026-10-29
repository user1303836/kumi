//! What the model thinks and what each answer costs: while it thinks, its reasoning's headline (in NOW, "thinking · 31s",
//! and on a step row of its own) until the answer speaks, never in the answer or what /copy takes; the header's
//! context meter from half full (or always, or never); and the quiet line under each finished answer: its time, its
//! changes and its tokens on an API key, or its share of the plan on a subscription.
#[path = "../support/models.rs"]
mod models;
#[path = "../support/tui_app.rs"]
mod support;
use kumi::tui::app::ContextMeter;
use models::FakeModels;
use serde_json::{json, Value};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

/// The catalog's models, `model` chosen, each reading 200k tokens at once.
fn models(model: &str) -> Rc<FakeModels> {
    let models = FakeModels::catalog();
    for list in models.lists.borrow_mut().values_mut() {
        for info in list.iter_mut() {
            info.context = Some(200_000.);
        }
    }
    *models.model.borrow_mut() = Some(model.into());
    models.signed_in.borrow_mut().insert(kumi_ai::providers::ProviderId::parse("anthropic").unwrap());
    models
}

fn usage(input: f64, output: f64) -> Value {
    json!({"type":"usage","inputTokens":input,"outputTokens":output,"cacheReadTokens":0.0,"cacheWriteTokens":0.0})
}

async fn asked(h: &Harness, text: &str) {
    h.type_text(&format!("{text}\r")).await;
    h.emit(json!({"type":"state","state":"running"}));
}

case!(a_long_think_shows_its_headline_in_now_and_on_its_row_until_the_answer_speaks, async {
    let copied = Rc::new(RefCell::new(Vec::<String>::new()));
    let copy = copied.clone();
    let h = Harness::with(120, 36, Rc::new(Control::default()), move |o| {
        o.copy = Some(Rc::new(move |text| {
            copy.borrow_mut().push(text.into());
            true
        }));
    });
    h.start().await;
    h.connect();
    asked(&h, "make the kick punch through the bass").await;
    h.emit(json!({"type":"reasoning","text":"**Weighing kick against bass**\n\nThe sub rings"}));
    h.emit(json!({"type":"reasoning","text":" 60 ms past each hit."}));
    let lines = h.screen();
    // NOW: "thinking" and its time, the headline beneath; the answer's own row says it too, as a step at work.
    assert!(lines.iter().any(|line| line.contains("NOW") && line.contains("● thinking · 0s")), "{}", lines.join("\n"));
    assert_eq!(lines.iter().filter(|line| line.contains("Weighing kick against bass")).count(), 2, "{}", lines.join("\n"));
    assert!(lines.iter().any(|line| line.contains("│") && line.contains("Weighing kick against bass")), "{}", lines.join("\n"));
    assert!(!has(&lines, "The sub rings") && !has(&lines, "**"), "one line, its title, without its Markdown");
    // The answer's first words end it: the headline goes, and it's never part of the answer.
    h.emit(json!({"type":"text","text":"Sidechain the bass to the kick."}));
    assert!(!has(&h.screen(), "Weighing"), "{}", h.screen().join("\n"));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":3100}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.type_text("/copy\r").await;
    assert_eq!(copied.borrow().last().map(String::as_str), Some("Sidechain the bass to the kick."));
    h.close().await;
});

case!(a_think_after_a_step_has_its_own_headline_and_time, async {
    let h = Harness::new(120, 36);
    h.start().await;
    h.connect();
    asked(&h, "check the low end").await;
    h.emit(json!({"type":"reasoning","text":"**Looking at the Set first**"}));
    h.emit(json!({"type":"tool-start","id":"t1","name":"live_discover"}));
    // A step at work: its own row, not the headline.
    assert!(!has(&h.screen(), "Looking at the Set first"), "{}", h.screen().join("\n"));
    h.emit(json!({"type":"tool-end","id":"t1","name":"live_discover","isError":false,"elapsedMs":300}));
    h.emit(json!({"type":"reasoning","text":"**Checking the sidechain**\n\nThe ratio"}));
    let lines = h.screen();
    assert!(lines.iter().any(|line| line.contains("│ ✓ looked at your Set")), "{}", lines.join("\n"));
    assert!(lines.iter().any(|line| line.contains("│") && line.contains("Checking the sidechain")), "{}", lines.join("\n"));
    assert!(!has(&lines, "Looking at the Set first"), "{}", lines.join("\n"));
    h.close().await;
});

case!(the_context_meter_shows_from_half_full_or_as_set, async {
    for (meter, input, shown) in [
        (ContextMeter::Auto, 81_000., None),
        (ContextMeter::Auto, 165_000., Some("context ▰▰▰▰▰▰▰▰▱▱ 83%")),
        (ContextMeter::Always, 81_000., Some("context ▰▰▰▰▱▱▱▱▱▱ 41%")),
        (ContextMeter::Off, 165_000., None),
    ] {
        let fake = models("anthropic/claude-sonnet-5-5");
        let h = Harness::with(120, 36, Rc::new(Control::default()), |o| {
            o.models = Some(fake.clone());
            o.context_meter = meter;
        });
        h.start().await;
        h.connect();
        asked(&h, "tighten the hats").await;
        assert!(!has(&h.screen(), "context"), "nothing before a call");
        h.emit(usage(input, 1_000.));
        let header = h.screen()[0].clone();
        match shown {
            Some(meter) => assert!(header.contains(meter), "{meter:?} in {header:?}"),
            None => assert!(!header.contains("context"), "{header:?}"),
        }
        // Past 80%, in the warning colour; below it, dim.
        if let Some(meter) = shown {
            let [r, g, b] = if input > 160_000. { kumi::tui::style::palette::WARN } else { kumi::tui::style::palette::DIM };
            let percent = &meter[meter.len() - 3..];
            assert!(h.styles_of(percent)[0].starts_with(&format!("{r},{g},{b}|")), "{:?}", h.styles_of(percent));
        }
        // A fresh conversation fills the context anew: the meter waits for its next call.
        h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
        h.emit(json!({"type":"state","state":"idle"}));
        h.type_text("/new\r").await;
        assert!(!h.screen()[0].contains("context"), "{}", h.screen()[0]);
        h.close().await;
    }
});

case!(an_answer_on_an_api_key_ends_with_its_time_changes_and_tokens, async {
    let fake = models("anthropic/claude-sonnet-5-5");
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.models = Some(fake.clone()));
    h.start().await;
    h.connect();
    asked(&h, "tighten the hats").await;
    for id in ["c1", "c2"] {
        h.emit(json!({"type":"change","change":{"id":id,"family":"tempo","title":format!("Tempo change {id}"),"state":"applied","at":1}}));
    }
    h.emit(json!({"type":"text","text":"Nudged the hats."}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed","usage":{"inputTokens":18_400.0,"outputTokens":1_200.0,"cacheReadTokens":16_100.0,"cacheWriteTokens":0.0}},"elapsedMs":6200}));
    h.emit(json!({"type":"state","state":"idle"}));
    h.has("6.2s · 2 changes · 18.4k in · 1.2k out");
    h.close().await;
});

case!(an_answer_on_a_subscription_ends_with_its_share_of_the_plan, async {
    let headers = |used: &str| -> BTreeMap<String, String> {
        [("x-codex-primary-used-percent", used), ("x-codex-primary-window-minutes", "300")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    kumi_ai::providers::usage::forget();
    kumi_ai::providers::usage::record("openai-codex", &headers("40"));
    let fake = models("openai-codex/gpt-6-astra");
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.models = Some(fake.clone()));
    h.start().await;
    h.connect();
    asked(&h, "tighten the hats").await;
    h.emit(json!({"type":"text","text":"Nudged the hats."}));
    kumi_ai::providers::usage::record("openai-codex", &headers("43"));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed","usage":{"inputTokens":18_400.0,"outputTokens":1_200.0,"cacheReadTokens":0.0,"cacheWriteTokens":0.0}},"elapsedMs":4000}));
    h.emit(json!({"type":"state","state":"idle"}));
    // No tokens on a subscription: what share of the plan's window it took.
    h.has("4.0s · 3% of your plan");
    assert!(!has(&h.screen(), "18.4k"), "{}", h.screen().join("\n"));
    h.close().await;
});

case!(a_window_not_known_yet_is_read_from_its_providers_list_once, async {
    // A local server's list comes only when asked: the meter reads it once a call has finished, and shows.
    let fake = models("anthropic/claude-sonnet-5-5");
    fake.unread.borrow_mut().insert("anthropic".into());
    let h = Harness::with(120, 36, Rc::new(Control::default()), |o| o.models = Some(fake.clone()));
    h.start().await;
    h.connect();
    asked(&h, "tighten the hats").await;
    h.emit(usage(165_000., 1_000.));
    h.wait_for("context ▰▰▰▰▰▰▰▰▱▱ 83%").await;
    h.emit(usage(170_000., 1_000.));
    h.has("context ▰▰▰▰▰▰▰▰▰▱ 86%");
    assert_eq!(fake.calls.borrow().iter().filter(|call| *call == "list:anthropic").count(), 1, "{:?}", fake.calls.borrow());
    h.close().await;
});
