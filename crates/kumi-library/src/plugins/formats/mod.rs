//! Third-party plug-ins' preset and saved-state formats, which Kumi decodes itself. Each plug-in's format is
//! data too: `plugin-formats/<plug-in>/` in the repo holds its IDs, where it keeps presets, and one folder per
//! format with `structure.json` (the wrapping and every field: type, unit, range, option meanings, the
//! parameters that move it, each marked verified or guessed) and `verified.json` (the plug-in versions
//! checked against it). The readers here unwrap the bytes; `structure` reads those files.
//!
//! Format evidence comes from saved files, initialized states and local plug-in/host investigation.
//! Manufacturer code and factory content are not bundled; Vital's published source is read without copying it.

pub mod bounded_json;
pub mod cbor;
pub mod juce_xml;
pub mod minimal_audio;
pub mod ozone12;
pub mod pulsar_modular;
pub mod serum2;
pub mod serum2_edit;
pub mod softube;
pub mod structure;
pub mod superior_drummer;
pub mod survey;
pub mod tree;
pub mod valhalla;
pub mod vital;
pub mod vstpreset;
pub mod xml;
pub mod zynaptiq;

/// Bytes a reader can't make sense of, and why.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FormatError(pub String);

impl FormatError {
    pub fn new(message: impl Into<String>) -> FormatError {
        FormatError(message.into())
    }
}

/// The largest committed decoded fixture is 322 KiB. Leave room for user patches,
/// while bounding both raw bytes and the tree's allocation amplification.
pub const MAX_DECODED_BYTES: usize = 2 * 1024 * 1024;
/// The largest fixture has 13,667 nodes including XML attributes or JSON keys.
pub const MAX_DECODED_ITEMS: usize = 65_536;
pub const MAX_DECODED_DEPTH: usize = 64;
