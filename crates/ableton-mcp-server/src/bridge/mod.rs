pub mod remote_adapter;
pub mod router;

// Live 11 adapter for extension-less operation
#[cfg(feature = "live-11")]
pub mod live11_adapter;

mod wire;

mod listeners;

#[cfg(not(feature = "live-11"))]
pub mod extension_channel;

#[cfg(not(feature = "live-11"))]
pub mod extension_launcher;

#[cfg(not(feature = "live-11"))]
pub mod extension_setup;

#[cfg(not(feature = "live-11"))]
pub mod live_extension_folders;
