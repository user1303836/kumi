//! The screen's colours as the terminal's change: a theme that follows the terminal (`system`, the setting's default)
//! turns light or dark as the terminal says its colours did. A theme the producer chose stays.

use super::super::{
    keys::Reply,
    style::{self, Theme, DARK, LIGHT},
};
use super::*;

impl TuiApp {
    /// What the terminal answered while Kumi runs: its colours turned dark or light (mode 2031's report), or a
    /// background it was asked for again.
    pub(super) fn terminal_replied(&self, reply: &Reply) {
        if self.0.options.theme.is_some() {
            return;
        }
        let theme = match reply {
            Reply::Scheme { dark } => {
                if *dark {
                    &DARK
                } else {
                    &LIGHT
                }
            }
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
        // Pictures up in the terminal go, and what's shown is drawn and uploaded afresh, as after the editor.
        let gone = std::mem::take(&mut self.0.state.borrow_mut().images).clear();
        self.0.tty.write(&gone);
        if let Some(map) = self.0.state.borrow_mut().map.as_mut() {
            map.rethemed();
        }
        self.0.scheduler.request();
    }
}
