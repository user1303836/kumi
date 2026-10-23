//! What every DAW's integration uses: reading parameter display text, finding samples on disk, and the change
//! engine's DAW-neutral parts.
pub mod changes;
pub mod display;
// Finding samples on disk (kumi-library, which finds the producer's sounds with it).
pub use kumi_library::samples;
pub mod steps;
pub mod undo;
