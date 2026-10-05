//! A spinner after a step's words while Kumi works ("Updating Live's Remote Script and the bridge… ⠹"),
//! so a step that takes a minute doesn't look stuck. Only in a terminal: piped, or with KUMI_UI=plain
//! (screen readers) or TERM=dumb, a step is just its line and nothing moves. It's the full-screen app's
//! thinking spinner, in characters every console font has where Kumi's icons are badges.
//!
//! The spinner's frames are a Tokio task, so it turns inside a `LocalSet`.

use std::cell::Cell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use tokio::task::JoinHandle;

use kumi_common::js;
use kumi_common::time::now_ms;

use crate::tui::activity::{activity_glyph, Activity};
use crate::tui::icons::{detect_icon_style, IconStyle};
use crate::tui::style::{detect_color_depth, os_release, process_platform, sgr, ColorDepth};
use crate::tui::tty::TtyOutput;

/// A spinner under way: `stop` ends it; a kept line stays as its words. Saying it twice does nothing.
pub struct Spinning {
    out: Option<Rc<dyn TtyOutput>>,
    line: String,
    keep: bool,
    timer: Option<JoinHandle<()>>,
    stopped: Cell<bool>,
}

impl Spinning {
    /// The spinner goes; a kept line stays as its words. Saying it twice does nothing.
    pub fn stop(&self) {
        if self.stopped.replace(true) {
            return;
        }
        if let Some(timer) = &self.timer {
            timer.abort();
        }
        if let Some(out) = &self.out {
            out.write(&format!("\r\u{1b}[2K{}", if self.keep { format!("{}\n", self.line) } else { String::new() }));
        }
    }
}

impl Drop for Spinning {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether `out` gets a spinner: a terminal, and plain lines weren't asked for.
pub fn spins(out: &dyn TtyOutput, env: &HashMap<String, String>) -> bool {
    out.is_tty() && env.get("KUMI_UI").map(String::as_str) != Some("plain") && env.get("TERM").map(String::as_str) != Some("dumb")
}

/// `line` with a spinner after it until stop(). Kept, the line stays once stopped, and is
/// all that's written where nothing spins; otherwise it shows only while spinning, for a wait nothing
/// else describes. Nothing else may write to `out` until it's stopped.
pub fn spin(out: Rc<dyn TtyOutput>, env: &HashMap<String, String>, line: &str, keep: bool) -> Spinning {
    if !spins(out.as_ref(), env) {
        if keep {
            out.write(&format!("{line}\n"));
        }
        return Spinning { out: None, line: line.to_string(), keep, timer: None, stopped: Cell::new(true) };
    }
    let plain = detect_icon_style(env, process_platform()) == IconStyle::Badges;
    let depth = detect_color_depth(env, process_platform(), &os_release());
    let started = now_ms();
    let draw = {
        let out = Rc::clone(&out);
        let line = line.to_string();
        move || {
            let glyph = activity_glyph(Activity::Think, (now_ms() - started) as f64, plain);
            // On one row, so each frame replaces the last: a line too wide for the window is shortened while it spins.
            let room = (out.columns().unwrap_or(80) - 3).max(8);
            let words = if js::string::utf16_len(&line) as i32 > room {
                format!("{}…", js::string::head(&line, (room - 1) as usize))
            } else {
                line.clone()
            };
            let shown =
                if depth == ColorDepth::None { glyph.text.clone() } else { format!("{}{}\u{1b}[0m", sgr(&glyph.style, depth), glyph.text) };
            out.write(&format!("\r\u{1b}[2K{words} {shown}"));
        }
    };
    draw();
    // Never what keeps Kumi running: the work it shows does that.
    let timer = tokio::task::spawn_local(async move {
        let mut ticks = tokio::time::interval(Duration::from_millis(100));
        ticks.tick().await;
        loop {
            ticks.tick().await;
            draw();
        }
    });
    Spinning { out: Some(out), line: line.to_string(), keep, timer: Some(timer), stopped: Cell::new(false) }
}

/// `line` (a step: "Copying the bridge…") with a spinner after it while `work` runs.
pub async fn step<T, F: Future<Output = T>>(out: Rc<dyn TtyOutput>, env: &HashMap<String, String>, line: &str, work: F, keep: bool) -> T {
    let spinning = spin(out, env, line, keep);
    let result = work.await;
    spinning.stop();
    result
}
