//! LYNX SHARE through its native authenticated loopback control endpoint.
pub mod client;
mod extensions;
mod integration;
mod provider;
mod tools;

pub use integration::LynxIntegration;
pub use provider::PROVIDER;
