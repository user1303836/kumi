//! The native `kumi` process entrypoint and command dispatch.
mod native;
pub use native::{help, main_with, run, CliIo};

/// Run the terminal application with each DAW's production integration.
pub fn main() -> i32 {
    main_with(kumi_daw::io::Answers::default())
}
