//! What a front end has, for what it offers: what the session can do ([`Capabilities`]), what the front end has itself
//! ([`Fronts`]), and the full-screen app's own (the mouse, copying as text is selected). The `/` menu, the help and the
//! keys all read it, so what they offer can't drift apart. Plain mode makes its own.
use kumi_agent::core::{
    commands::{Command, Fronts},
    protocol::Capabilities,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Has {
    pub can: Capabilities,
    pub fronts: Fronts,
    /// Kumi has the mouse (`mouse`); off, the terminal keeps it, and the wheel and Kumi's clicks aren't there.
    pub mouse: bool,
    /// Text copies as the mouse lets go of it (`copyOnSelect`).
    pub copy_on_select: bool,
}

impl Has {
    /// Whether the `/` menu and the help offer `command` here.
    pub fn offers(&self, command: &Command) -> bool {
        command.offered(&self.can, &self.fronts)
    }
}
