//! What Kumi's crates share, so the Rust port behaves as the TypeScript did where the language
//! shows: JSON written as JavaScript writes it (`js::json`), strings measured as JavaScript measures
//! them (`js::string`), cancellation in the shape of `AbortSignal` (`abort`), clocks (`time`), which paths
//! never to open (`path`), when another process started (`process`), and this process's environment and arguments
//! read without panicking on what isn't Unicode (`env`).

pub mod abort;
pub mod bridge;
pub mod env;
pub mod js;
pub mod path;
pub mod process;
pub mod time;

mod locale;
