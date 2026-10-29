//! The `/` menu: what it finds for what's typed (a name, then the words of what a command does), the commands sent
//! lately first, and a window around the one chosen when they don't all fit above the box.
#[path = "../support/models.rs"]
mod models;
#[path = "../support/tui_app.rs"]
mod support;
use kumi::history::open_input_history;
use std::{cell::RefCell, rc::Rc};
use support::*;

#[tokio::test(flavor = "current_thread")]
async fn the_menu_finds_a_command_by_the_words_of_what_it_does() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new(120, 36);
            h.start().await;
            h.connect();
            // Nothing is called "hist…", but undoing a change takes it off HISTORY: chosen, with what it does.
            h.type_text("/hist").await;
            h.has("/undo");
            h.has("Undo Kumi's last change");
            h.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_menu_lists_the_commands_sent_lately_first() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::with(120, 36, Rc::new(Control::default()), |o| {
                o.history = Some(Rc::new(RefCell::new(open_input_history(None, vec![]))));
            });
            h.start().await;
            h.connect();
            h.type_text("/status\r").await;
            // Sent once: first in the menu now, so it's the one chosen, with what it does beside it.
            h.type_text("/").await;
            h.has("What Kumi is connected to");
            h.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_menu_taller_than_the_room_shows_a_window_around_the_one_chosen() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // A short screen has less room above the box than this session has commands.
            let h = Harness::new(100, 14);
            h.start().await;
            h.connect();
            h.type_text("/").await;
            h.has("/new");
            h.has("Start a fresh conversation");
            h.has("↓");
            h.has("more");
            // Up from the first goes round to the last: the window follows it, and what's above is said.
            h.type_text("\x1b[A").await;
            h.has("/quit");
            h.has("Close Kumi");
            h.has("↑");
            h.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_command_found_by_what_it_does_goes_in_the_box_before_enter_runs_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new(120, 36);
            h.start().await;
            h.connect();
            // /history names no command: Enter puts /undo's name in the box, and undoes nothing yet.
            h.type_text("/history\r").await;
            h.has("/undo");
            assert!(!has(&h.screen(), "There's nothing of Kumi's to undo."), "{}", h.screen().join("\n"));
            // Enter again runs what the box says now.
            h.type_text("\r").await;
            h.has("There's nothing of Kumi's to undo.");
            h.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_command_found_by_what_it_does_runs_once_chosen() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new(120, 36);
            h.start().await;
            h.connect();
            // Chosen with ↓, it's the producer's choice: Enter runs it.
            h.type_text("/history").await;
            h.type_text("\x1b[B").await;
            h.type_text("\r").await;
            h.has("There's nothing of Kumi's to undo.");
            h.close().await;
        })
        .await;
}

/// The menu's rows as the screen shows them: the lines, from the menu's top to the box.
fn menu_rows(h: &Harness) -> Vec<String> {
    h.screen().into_iter().filter(|line| line.starts_with("   ") && !line.trim().is_empty()).map(|line| line.trim().to_string()).collect()
}

#[tokio::test(flavor = "current_thread")]
async fn tab_completes_a_commands_own_words() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new(120, 36);
            h.start().await;
            h.connect();
            // Once the command and a space are typed, its words; what's typed of one narrows them.
            h.type_text("/export ").await;
            for word in ["copy", "md", "html", "json"] {
                h.has(word);
            }
            h.type_text("h").await;
            assert!(!has(&h.screen(), "json"), "{}", h.screen().join("\n"));
            h.type_text("\t").await;
            assert_eq!(h.app.draft(), "/export html");
            h.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_palettes_name_is_offered_after_use_read_once_and_enter_runs_only_what_was_named() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let control = Rc::new(Control::default());
            control.set("palette", serde_json::json!(true));
            control
                .set("palettes", serde_json::json!([{"name":"Kicks","items":12,"current":true},{"name":"Pads","items":1,"current":false}]));
            let h = Harness::with(120, 36, control.clone(), |_| {});
            h.start().await;
            h.connect();
            h.type_text("/palette ").await;
            h.has("rename");
            // use's word is a palette: each with how many sounds it holds, this session's marked.
            h.type_text("use ").await;
            h.wait_for("12 sounds · this session's").await;
            h.has("1 sound");
            h.type_text("pa").await;
            h.type_text("q\x7f").await;
            assert!(!has(&menu_rows(&h), "Kicks"), "{:?}", menu_rows(&h));
            // Read once for the word, however many keys and frames came.
            assert_eq!(h.calls().iter().filter(|call| *call == "palette-names").count(), 1, "{:?}", h.calls());
            // Partly typed, Enter puts the palette in and runs nothing; whole, it runs.
            h.type_text("\r").await;
            assert_eq!(h.app.draft(), "/palette use Pads");
            assert!(!h.calls().iter().any(|call| call.starts_with("palette:")), "{:?}", h.calls());
            h.type_text("\r").await;
            h.wait_for_call("palette:use Pads").await;
            // A word not begun runs the line as it stands.
            h.type_text("/palette \r").await;
            h.wait_for_call("palette:").await;
            h.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn resume_offers_the_kept_conversations_by_what_was_asked_in_them() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let control = Rc::new(Control::default());
            let now = kumi_common::time::now_ms();
            *control.conversations.borrow_mut() = serde_json::from_value(serde_json::json!([
                {"id":"now001","savedAt":now,"first":"add a hi-hat groove","turns":2,"current":true},
                {"id":"old001","savedAt":now - 2 * 3_600_000,"first":"make the bass wider","turns":5,"current":false},
                {"id":"old002","savedAt":now - 30 * 3_600_000,"first":"tighten the drums","turns":3,"current":false,"name":"Drum pass"}
            ]))
            .unwrap();
            let h = Harness::with(120, 36, control, |_| {});
            h.start().await;
            h.connect();
            h.type_text("/resume ").await;
            h.wait_for("make the bass wider").await;
            // The one going on isn't offered: /resume says that's this one.
            assert!(!has(&menu_rows(&h), "hi-hat"), "{:?}", menu_rows(&h));
            h.has("2 hours ago");
            // Found by what was asked in it; tab puts in its id, or its name where it has one.
            h.type_text("bass").await;
            h.type_text("\t").await;
            assert_eq!(h.app.draft(), "/resume old001");
            h.type_text("\x15/resume dru\t").await;
            assert_eq!(h.app.draft(), "/resume Drum pass");
            h.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn slots_offers_a_slot_then_the_models_it_takes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use kumi_ai::auth::store::{open_credential_store, CredentialStore};
            use kumi_listening::slots::{self, SlotsContext};
            let folder = tempfile::tempdir().unwrap();
            let control = Rc::new(Control::default());
            *control.slots.borrow_mut() = Some(Rc::new(SlotsContext {
                file: slots::file_in(folder.path()),
                store: Rc::new(open_credential_store(folder.path().join("auth.json"))) as Rc<dyn CredentialStore>,
                env: Default::default(),
                profile: None,
            }));
            let h = Harness::with(120, 36, control, |_| {});
            h.start().await;
            h.connect();
            h.type_text("/slots li\t").await;
            // The slot, and a space: the models it takes follow at once, each with what it is.
            assert_eq!(h.app.draft(), "/slots listening ");
            h.has("Gemini (its newest model that takes audio)");
            h.has("off: the meters alone");
            h.type_text("op\t").await;
            assert_eq!(h.app.draft(), "/slots listening openai");
            // After back, a slot again.
            h.type_text("\x15/slots back em\t").await;
            assert_eq!(h.app.draft(), "/slots back embeddings");
            h.close().await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn model_offers_the_signed_in_providers_models_and_chooses_one_by_its_id() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fake = models::FakeModels::catalog();
            for provider in ["anthropic", "openai-codex"] {
                fake.signed_in.borrow_mut().insert(kumi_ai::providers::ProviderId::parse(provider).unwrap());
            }
            let h = Harness::with(120, 36, Control::with_models(fake.clone()), |_| {});
            h.start().await;
            h.connect();
            h.type_text("/model ").await;
            h.wait_for("anthropic/claude-sonnet-5-5").await;
            h.has("GPT-6 Astra · chosen");
            // Found by a word of its name; tab puts in its id.
            h.type_text("haiku\t").await;
            assert_eq!(h.app.draft(), "/model anthropic/claude-haiku-4-5");
            // The id whole, Enter chooses it, as the picker does.
            h.type_text("\r").await;
            h.wait_for("Kumi talks to Claude Haiku 4.5 from your next message").await;
            h.close().await;
        })
        .await;
}
