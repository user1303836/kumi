//! Live's own parts of the app: finding, opening and checking Live, its bridge (the Remote Script), Kumi's Live
//! Extension, and Willington's bindings.
pub mod answers;
pub mod bridge_setup;
pub mod doctor;
pub mod install;
pub mod live_app;
pub mod live_extension;
pub mod provider;
pub mod report;
pub mod setup;
pub mod willington;

pub use answers::AbletonFactory;
