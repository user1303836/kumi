//! Live as Kumi's app works with it, through kumi-daw: Live's provider (finding, opening and checking Live, its bridge
//! and the Remote Script put in, brought up to date, rolled back and taken out, Kumi's Live Extension, Willington's
//! bindings, Live's side of first-run setup), its part of the doctor and the report, and the answers its tests give.
pub mod answers;
pub mod bridge_setup;
pub mod doctor;
pub mod folders;
pub mod install;
pub mod live_app;
pub mod live_extension;
pub mod provider;
pub mod report;
pub mod setup;
pub mod willington;

pub use answers::AbletonFactory;
