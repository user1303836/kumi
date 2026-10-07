//! Third-party plug-ins' preset and saved-state formats, which Kumi decodes itself. Each plug-in's format is
//! data too: `plugin-formats/<plug-in>/` in the repo holds its IDs, where it keeps presets, and one folder per
//! format with `structure.json` (the wrapping and every field: type, unit, range, option meanings, the
//! parameters that move it, each marked verified or guessed) and `verified.json` (the plug-in versions
//! checked against it). The readers here unwrap the bytes; `structure` reads those files.
//!
//! What's here learns from files and from how the plug-ins behave, never from their code. Vital's format
//! is read from its published source, without copying it.

pub mod cbor;
pub mod live_set;
pub mod ozone12;
pub mod serum2;
pub mod structure;
pub mod survey;
pub mod tree;
pub mod vital;
pub mod xml;

/// Bytes a reader can't make sense of, and why.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FormatError(pub String);

impl FormatError {
    pub fn new(message: impl Into<String>) -> FormatError {
        FormatError(message.into())
    }
}

/// The most a compressed body may grow to when decoded, so a hostile file can't exhaust memory.
pub const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
