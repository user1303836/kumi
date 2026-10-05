use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::Duration;

use kumi::spinner::{spin, spins, step};
use kumi::tui::tty::TtyOutput;

/// An output that's a terminal (or not), `columns` wide, and everything written to it.
struct Output {
    is_tty: bool,
    columns: i32,
    text: RefCell<String>,
}

impl TtyOutput for Output {
    fn is_tty(&self) -> bool {
        self.is_tty
    }
    fn columns(&self) -> Option<i32> {
        Some(self.columns)
    }
    fn rows(&self) -> Option<i32> {
        None
    }
    fn write(&self, data: &str) {
        self.text.borrow_mut().push_str(data);
    }
}

fn output(is_tty: bool, columns: i32) -> Rc<Output> {
    Rc::new(Output { is_tty, columns, text: RefCell::new(String::new()) })
}

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
}

fn terminal() -> HashMap<String, String> {
    env(&[("TERM_PROGRAM", "WezTerm"), ("COLORTERM", "truecolor")])
}

static ANSI: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\u{1b}\[[0-9;]*[A-Za-z]").unwrap());

fn plain_text(text: &str) -> String {
    ANSI.replace_all(text, "").into_owned()
}

fn frames(text: &str) -> Vec<String> {
    text.split("\r\u{1b}[2K").filter(|frame| !frame.is_empty()).map(str::to_string).collect()
}

async fn local<F: Future>(future: F) -> F::Output {
    tokio::task::LocalSet::new().run_until(future).await
}

#[tokio::test]
async fn piped_a_step_is_just_its_line_a_quiet_one_says_nothing_and_nothing_moves() {
    local(async {
        let piped = output(false, 80);
        assert_eq!(step(piped.clone(), &terminal(), "Copying the bridge…", async { 42 }, true).await, 42);
        assert_eq!(step(piped.clone(), &terminal(), "Checking what changes…", async { "planned" }, false).await, "planned");
        assert_eq!(*piped.text.borrow(), "Copying the bridge…\n");
        assert!(!spins(piped.as_ref(), &terminal()));
    })
    .await;
}

#[tokio::test]
async fn in_a_terminal_a_spinner_turns_after_the_line_while_the_work_runs_and_the_line_stays_without_it() {
    local(async {
        let tty = output(true, 80);
        let result = step(
            tty.clone(),
            &terminal(),
            "Updating Live's Remote Script and the bridge…",
            async {
                tokio::time::sleep(Duration::from_millis(350)).await;
                "applied"
            },
            true,
        )
        .await;
        assert_eq!(result, "applied");
        let frames = frames(&tty.text.borrow());
        assert!(frames.len() >= 3, "it turned ({} frames)", frames.len());
        let turning = regex::Regex::new("^Updating Live's Remote Script and the bridge… [⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]$").unwrap();
        for frame in &frames[..frames.len() - 1] {
            assert!(turning.is_match(&plain_text(frame)), "{frame:?}");
        }
        assert!(
            frames[..frames.len() - 1].iter().map(|frame| plain_text(frame)).collect::<std::collections::HashSet<_>>().len() > 1,
            "the glyph changes"
        );
        assert!(frames[0].contains("\u{1b}[0;38;2;134;227;181m"), "in Kumi's accent colour");
        assert_eq!(frames.last().unwrap(), "Updating Live's Remote Script and the bridge…\n", "the line stays, without the spinner");
    })
    .await;
}

#[tokio::test]
async fn a_quiet_step_shows_only_while_it_runs_and_leaves_its_row_empty() {
    local(async {
        let tty = output(true, 80);
        step(tty.clone(), &terminal(), "Checking whether Live is open…", async { false }, false).await;
        assert_eq!(plain_text(&tty.text.borrow()), "\rChecking whether Live is open… ⠋\r");
        assert!(tty.text.borrow().ends_with("\r\u{1b}[2K"));
    })
    .await;
}

#[tokio::test]
async fn work_that_fails_stops_the_spinner_keeps_the_line_and_passes_the_failure_on() {
    local(async {
        let tty = output(true, 80);
        let failed: Result<(), String> =
            step(tty.clone(), &terminal(), "Downloading Kumi 9.9.9…", async { Err("the download failed (503)".to_string()) }, true).await;
        assert!(failed.unwrap_err().contains("503"));
        assert!(tty.text.borrow().ends_with("\r\u{1b}[2KDownloading Kumi 9.9.9…\n"));
    })
    .await;
}

#[tokio::test]
async fn plain_lines_kumi_ui_plain_term_dumb_never_spin_even_in_a_terminal() {
    local(async {
        let mut plain = terminal();
        plain.insert("KUMI_UI".to_string(), "plain".to_string());
        for env in [plain, env(&[("TERM", "dumb")])] {
            let tty = output(true, 80);
            step(tty.clone(), &env, "Packing the bridge…", async { tokio::time::sleep(Duration::from_millis(150)).await }, true).await;
            assert_eq!(*tty.text.borrow(), "Packing the bridge…\n", "{env:?}");
        }
    })
    .await;
}

#[tokio::test]
async fn where_kumis_icons_are_badges_the_spinner_is_plain_characters_and_no_color_leaves_it_uncoloured() {
    local(async {
        let tty = output(true, 80);
        step(
            tty.clone(),
            &env(&[("KUMI_ICONS", "badges"), ("NO_COLOR", "1")]),
            "Installing its package…",
            async { tokio::time::sleep(Duration::from_millis(250)).await },
            true,
        )
        .await;
        let all = frames(&tty.text.borrow());
        let frames = &all[..all.len() - 1];
        assert!(frames.len() >= 2);
        let turning = regex::Regex::new(r"^Installing its package… [|/\\-]$").unwrap();
        for frame in frames {
            assert!(turning.is_match(frame), "{frame:?}");
        }
    })
    .await;
}

#[tokio::test]
async fn a_line_wider_than_the_window_is_shortened_while_it_spins_and_kept_whole() {
    local(async {
        let tty = output(true, 31);
        let line = "Waiting for Live… (Enter or Ctrl-C stops waiting; nothing else depends on it)";
        let spinning = spin(tty.clone(), &terminal(), line, true);
        let frame = plain_text(&tty.text.borrow());
        let frame = frame.strip_prefix('\r').unwrap_or(&frame).to_string();
        assert_eq!(frame.chars().count(), 30, "within the window, so each frame replaces the last");
        assert!(regex::Regex::new(r"^Waiting for Live… \(Enter or… ⠋$").unwrap().is_match(&frame), "{frame:?}");
        spinning.stop();
        spinning.stop();
        assert!(tty.text.borrow().ends_with(&format!("\r\u{1b}[2K{line}\n")));
        assert_eq!(tty.text.borrow().split(line).count(), 2, "stopping twice says it once");
    })
    .await;
}
