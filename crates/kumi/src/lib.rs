/// Kumi's release, as `kumi --version`, the doctor and updates say it: this package's version, which a release moves.
/// `main` gives it to `kumi_types::version::set` first thing, so the crates below say the same.
pub const KUMI_VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod awake;
pub mod cli;
pub mod config;
pub mod daw;
pub mod design;
pub mod doctor;
pub mod export;
pub mod has;
pub mod history;
pub mod input;
pub mod install;
pub mod library;
pub mod login;
pub mod map;
pub mod map_server;
pub mod model_lists;
pub mod models;
pub mod print;
pub mod process;
pub mod report;
pub mod serve;
pub mod spinner;
pub mod terminal;
pub mod text;
pub mod tui;
pub mod update;
pub mod voice;
pub mod whats_new;
pub mod workers;
// Live's own parts of the app, at the paths they always had.
