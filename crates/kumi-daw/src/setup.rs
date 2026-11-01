//! The DAW's side of first-run setup, which the app's setup screen can't do on its own: the DAW the app, and Kumi's
//! bridge in it.
use crate::provider::Installed;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_types::errors::RuntimeError;
use std::time::Duration;

/// The session as the DAW's side of setup reaches it: setup asks it only this.
#[async_trait(?Send)]
pub trait SetupSession {
    /// Connect again, through the bridge just put in place: its configuration is in the `bridge` cell
    /// [`DawProvider::setup`](crate::provider::DawProvider::setup) is given, which setup sets first unless it was given.
    /// An error while the session can't yet (it's still starting), so setup asks again shortly.
    async fn reconnect(&self) -> Result<(), RuntimeError>;
    /// The saved project open in the DAW, by its file, once the session has read it.
    fn open_set(&self) -> Option<String>;
}

/// What a DAW opens by itself when it starts with nothing asked, going by its own settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reopens {
    /// Kumi can't tell, or the DAW starts with something new: a saved project is in its list of recent ones.
    Unknown,
    /// All it had open as it quit.
    Everything,
    /// Only the project it had in front as it quit, if that one was saved.
    Front,
}

/// The DAW's side of setup, which the app can't do on its own: the DAW the app, and Kumi's bridge in it. Its methods
/// keep Live's names (the app's Live side and setup's tests implement them).
#[async_trait(?Send)]
pub trait DawSetup {
    /// The open Live's app (a `.app` on macOS, an `.exe` on Windows), if Live is open.
    async fn open_live(&self) -> Option<String>;
    /// Ask Live to quit as its own menu does, so unsaved work makes it ask to save first. Returns once asked; false
    /// when the system didn't let Kumi ask (macOS's Automation off for this terminal), so the producer quits Live.
    async fn ask_to_quit(&self) -> bool;
    /// Wait until Live has closed: true once it has, false when `stop` comes first.
    async fn closed(&self, stop: &Signal) -> bool;
    /// Put Kumi's bridge in place, with Live closed: the bridge's version, or what went wrong, in words.
    async fn install(&self) -> Result<String, String>;
    /// Open Live: `app`, the one that was open, or else the newest one installed; with `set`, the Set it had open.
    async fn start(&self, app: Option<String>, set: Option<String>) -> bool;
    /// Connect the session to Live through the bridge just put in place.
    async fn connect(&self);
    /// Live's version ("12.4"), when it can be read from its app.
    async fn version(&self, app: Option<String>) -> Option<String>;
    /// Which Live is on this computer, from its files: whether one can take the bridge yet.
    async fn installed(&self) -> Installed {
        Installed::Ready
    }
    /// The saved Set open in Live, when the session has read it (through a bridge already there).
    fn open_set(&self) -> Option<String> {
        None
    }
    /// What the DAW opens by itself as setup's restart starts it, once it has closed.
    fn reopens(&self) -> Reopens {
        Reopens::Unknown
    }
    /// Whether Live runs on this computer at all (it doesn't on Linux): without it, setup only signs in.
    fn here(&self) -> bool {
        true
    }
    /// The app macOS names when it asks whether Kumi may ask Live to quit (the terminal); None where nothing asks.
    fn asks_permission(&self) -> Option<String> {
        None
    }
    /// Where Live keeps its Settings, said once the Control Surface step has waited a while.
    fn settings_menu(&self) -> Option<String> {
        None
    }
    /// Whether Kumi can ask the DAW to quit (Live's own menu, through macOS): when it can't, the producer quits it.
    fn can_ask_to_quit(&self) -> bool {
        true
    }
    /// How long Live gets to quit once asked, before Kumi says it's still open (its save dialog
    /// cancelled, say) and offers to ask again.
    fn quit_wait(&self) -> Duration {
        Duration::from_secs(15)
    }
    /// How long the Control Surface step waits before it says where Live's Settings are.
    fn surface_wait(&self) -> Duration {
        Duration::from_secs(45)
    }
}
