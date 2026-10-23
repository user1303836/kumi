//! Answers to what Live's steps would otherwise find out for themselves: each a type a test gives in
//! `kumi_daw::io::Answers` (which Lives are installed, whether Live runs, a stand-in for Live's integration).
use super::{doctor::PortOwner, live_app::InstalledLive};
use futures::future::LocalBoxFuture;
use kumi_agent::core::contracts::Integration;
use kumi_daw::{check::Check, process::AsyncBool};
use kumi_daw_ableton::AbletonOptions;
use kumi_types::errors::RuntimeError;
use std::rc::Rc;

/// Makes Live's integration from its options: a test's stand-in in place of kumi-daw-ableton's.
#[derive(Clone)]
pub struct AbletonFactory(pub Rc<dyn Fn(AbletonOptions) -> Rc<dyn Integration>>);

/// Whether Live runs.
#[derive(Clone)]
pub struct LiveRunning(pub AsyncBool);

/// The Lives installed here.
#[derive(Clone)]
pub struct Lives(pub Rc<dyn Fn() -> Vec<InstalledLive>>);

/// Whether Live runs its own Extension Host, which it does with Developer Mode off.
#[derive(Clone)]
pub struct ExtensionHost(pub AsyncBool);

/// Who listens on one of this computer's ports (Live's bridge's).
#[derive(Clone)]
pub struct PortOwners(pub Rc<dyn Fn(u16) -> LocalBoxFuture<'static, PortOwner>>);

/// The doctor's check of Kumi's hands in Live's menus.
#[derive(Clone)]
pub struct Hands(pub Rc<dyn Fn() -> LocalBoxFuture<'static, Result<Option<Check>, RuntimeError>>>);

/// The version of the Node a bridge's configuration starts a JavaScript bridge with.
#[derive(Clone)]
pub struct NodeVersions(pub Rc<dyn Fn(String) -> LocalBoxFuture<'static, Option<String>>>);

/// Live's log files, newest first, for `kumi report`.
#[derive(Clone)]
pub struct LiveLogs(pub Rc<dyn Fn() -> LocalBoxFuture<'static, Result<Vec<String>, RuntimeError>>>);

/// `kumi bridge` as an update's last step, from the app folder or checkout it names: its exit code.
#[derive(Clone)]
pub struct BridgeAfterUpdate(pub Rc<dyn Fn(String) -> LocalBoxFuture<'static, i32>>);

/// A run whose only answer is a stand-in for Live's integration (`kumi::cli::run(io, factory)`).
impl From<AbletonFactory> for kumi_daw::io::Answers {
    fn from(factory: AbletonFactory) -> Self {
        Self::default().with(factory)
    }
}
