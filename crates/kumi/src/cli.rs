//! The native `kumi` process entrypoint and command dispatch.
mod native;
pub use native::{help, main_with, run, AbletonFactory, CliIo};

/// Run the terminal application with the production Ableton integration.
pub fn main() -> i32 {
    main_with(std::rc::Rc::new(kumi_runtime::integrations::ableton::create_ableton_integration))
}
