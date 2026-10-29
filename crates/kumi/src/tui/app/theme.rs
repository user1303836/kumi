//! The screen's colours as the terminal's change: a theme that follows the terminal (`system`, the setting's default)
//! turns light or dark as the terminal says its colours did. A theme the producer chose stays.

use super::super::{
    keys::Reply,
    style::{self, Theme, DARK, LIGHT},
};
use super::*;

/// OSC 11's question: the terminal's background, which it answers as `Reply::Color { which: 11 }`.
const ASK_BACKGROUND: &str = "\u{1b}]11;?\u{1b}\\";

impl TuiApp {
    /// What the terminal answered while Kumi runs: its look turned dark or light (mode 2031's report), or its
    /// background, asked for again.
    pub(super) fn terminal_replied(&self, reply: &Reply) {
        if self.0.options.theme.is_some() {
            return;
        }
        let theme = match reply {
            // A report says the terminal's look changed, but in some it's the system's light or dark rather than the
            // terminal's own colours (Ghostty's report under a system set to Auto, with one dark theme): its background is
            // asked for, and that decides. Where it said none as Kumi started, the report is all there is.
            Reply::Scheme { .. } if self.0.profile.background.is_some() => {
                self.0.tty.write(ASK_BACKGROUND);
                return;
            }
            Reply::Scheme { dark: true } => &DARK,
            Reply::Scheme { dark: false } => &LIGHT,
            Reply::Color { which: 11, rgb: Some(background) } => style::for_background(Some(*background)),
            _ => return,
        };
        self.switch_theme(theme);
    }

    /// The screen in another theme: what was drawn in the last one's colours (a change's pictures, the map's points) is
    /// drawn again, and the renderer writes the whole screen anew.
    fn switch_theme(&self, theme: &'static Theme) {
        if style::theme().name == theme.name {
            return;
        }
        style::set_theme(theme);
        self.redraw_pictures();
        if let Some(map) = self.0.state.borrow_mut().map.as_mut() {
            map.rethemed();
        }
        self.0.scheduler.request();
    }
}
