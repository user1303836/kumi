//! A few words in the composer's hint row for a moment, in its hint's place: what the producer just did took
//! ("Copied 3 lines"), without a line in the conversation.
use super::*;

/// How long a toast shows.
pub(super) const TOAST_MS: f64 = 3000.;

pub(super) struct Toast {
    text: String,
    tone: NoticeTone,
    /// When it goes (perf time).
    until: f64,
}

impl TuiApp {
    /// Says `text` in the composer's hint row for a moment; a newer toast takes its place. A warning shows in the
    /// warning colour.
    pub(super) fn toast(&self, text: &str, tone: NoticeTone) {
        let text = self.clean(&text.replace('\n', " "), 200);
        self.0.state.borrow_mut().toast = Some(Toast { text, tone, until: perf_now() + TOAST_MS });
        self.0.scheduler.request();
    }

    /// The toast showing at `now`, if one is: the frame that draws it asks for the redraw that takes it away.
    pub(super) fn toasted(&self, now: f64) -> Option<(String, NoticeTone)> {
        let toast = {
            let mut state = self.0.state.borrow_mut();
            if state.toast.as_ref().is_some_and(|toast| now >= toast.until) {
                state.toast = None;
            }
            state.toast.as_ref().map(|toast| (toast.text.clone(), toast.tone, toast.until))
        };
        let (text, tone, until) = toast?;
        self.wake_at(until, now);
        Some((text, tone))
    }

    /// Whether a toast shows now: setup and the map draw no composer, and a list's own hint (a countdown to Kumi's pick)
    /// keeps its row.
    pub(super) fn toast_shows(&self) -> bool {
        if self.setup_active() || self.map_shown() {
            return false;
        }
        let top = self.0.state.borrow().overlays.top();
        !top.is_some_and(|top| top.borrow().hint_stays(self))
    }

    /// Takes the toast away before its time.
    pub(super) fn end_toast(&self) {
        if self.0.state.borrow_mut().toast.take().is_some() {
            self.0.scheduler.request();
        }
    }
}
