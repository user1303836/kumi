//! Live's own parts of the app: finding, opening and checking Live, its bridge (the Remote Script), Kumi's Live
//! Extension, and Willington's bindings.
pub mod bridge_setup;
pub mod doctor;
pub mod install;
pub mod live_app;
pub mod live_extension;
pub mod provider;
pub mod report;
pub mod setup;
pub mod willington;

use kumi_daw_ableton::AbletonOptions;
use kumi_runtime::core::contracts::Integration;
use std::rc::Rc;

/// Makes Live's integration from its options: the app's own, or a test's stand-in.
pub type AbletonFactory = Rc<dyn Fn(AbletonOptions) -> Rc<dyn Integration>>;
