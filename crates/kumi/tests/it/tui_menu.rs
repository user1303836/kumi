//! The `/` menu: what it finds for what's typed (a name, then the words of what a command does), the commands sent
//! lately first, and a window around the one chosen when they don't all fit above the box.
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
