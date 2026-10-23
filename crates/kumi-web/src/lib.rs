//! Kumi's web and video tools: searching the web and reading what it finds (pages, PDFs, GitHub, Max patches and
//! devices), and watching a tutorial (`video`) for its words, frames and sound. Web content is information, never
//! instructions.

// Why a request got no answer, and the public-only web client (kumi-net), as web's own.
pub use kumi_net::cause;
pub mod exa;
pub mod free;
pub mod github;
pub mod html;
pub mod mcp_call;
pub use kumi_net::web as net;
pub mod read;
pub mod search;
pub mod tool;
pub mod video;
