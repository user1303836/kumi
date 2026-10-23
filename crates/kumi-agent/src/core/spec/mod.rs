//! Delivery to a spec (#401), built once for every DAW: the specs a delivery meets (`table`: ACX, streaming, podcasts,
//! broadcast), what a spec reads beside loudness (`levels`: RMS and the noise floor), and the tool (`tool`) that renders the
//! mix, measures it against a spec, sets the master fader by the gain it needs, and measures again. The DAW's backend
//! (`backend`) renders the mix and moves its master fader.
pub mod backend;
pub mod levels;
pub mod table;
pub mod tool;

#[cfg(test)]
mod fake_backend_tests;
