//! Product-specific initialized VST3 states. Standard fixture files remain
//! readable in plugin-formats; build.rs compresses their embedded copies.

use std::io::Read;
use std::sync::LazyLock;

include!(concat!(env!("OUT_DIR"), "/plugin_templates.rs"));

static TEMPLATES: LazyLock<Vec<(&'static str, Vec<u8>)>> = LazyLock::new(|| {
    COMPRESSED_TEMPLATES
        .iter()
        .map(|(plugin, bytes)| {
            let mut state = Vec::new();
            flate2::read::ZlibDecoder::new(*bytes)
                .take(super::formats::MAX_DECODED_BYTES as u64 + 1)
                .read_to_end(&mut state)
                .expect("bundled initialized state decompresses");
            assert!(state.len() <= super::formats::MAX_DECODED_BYTES, "bundled initialized state exceeds its size budget");
            (*plugin, state)
        })
        .collect()
});

static VERSION_TEMPLATES: LazyLock<Vec<(&'static str, &'static str, Vec<u8>)>> = LazyLock::new(|| {
    COMPRESSED_VERSION_TEMPLATES
        .iter()
        .map(|(plugin, version, bytes)| {
            let mut state = Vec::new();
            flate2::read::ZlibDecoder::new(*bytes)
                .take(super::formats::MAX_DECODED_BYTES as u64 + 1)
                .read_to_end(&mut state)
                .expect("bundled version state decompresses");
            assert!(state.len() <= super::formats::MAX_DECODED_BYTES, "bundled version state exceeds its size budget");
            (*plugin, *version, state)
        })
        .collect()
});

pub fn initial(plugin: &str) -> Option<&'static [u8]> {
    TEMPLATES.iter().find(|(id, _)| *id == plugin).map(|(_, bytes)| bytes.as_slice())
}

/// An initialized state captured from an exact older build. Some products append
/// parameters between releases; their new default must not reach an older host.
pub fn initial_for_version(plugin: &str, version: &str) -> Option<&'static [u8]> {
    VERSION_TEMPLATES.iter().find(|(id, captured, _)| *id == plugin && *captured == version).map(|(_, _, bytes)| bytes.as_slice())
}

/// Whether a product has a creation template, without decompressing any state.
pub fn has_initial(plugin: &str) -> bool {
    COMPRESSED_TEMPLATES.iter().any(|(id, _)| *id == plugin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_reader_template_limits_agree() {
        assert_eq!(MAX_TEMPLATE_BYTES, crate::plugins::formats::MAX_DECODED_BYTES);
        for (plugin, _) in COMPRESSED_TEMPLATES {
            assert!(has_initial(plugin));
        }
        assert!(!has_initial("not-a-built-in-product"));
    }
}
