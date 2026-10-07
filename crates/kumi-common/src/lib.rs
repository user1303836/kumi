//! What Kumi's crates share, so the Rust port behaves as the TypeScript did where the language
//! shows: JSON written as JavaScript writes it (`js::json`), strings measured as JavaScript measures
//! them (`js::string`), cancellation in the shape of `AbortSignal` (`abort`), clocks (`time`), and which paths
//! never to open (`path`).

pub mod abort;
pub mod bridge;
pub mod js;
pub mod path;
pub mod time;

mod locale;
