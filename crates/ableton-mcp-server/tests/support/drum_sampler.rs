#![allow(dead_code)]

use std::fs;
use std::io::Write;

use flate2::write::GzEncoder;
use flate2::Compression;

/// The shape of Live 12's default Drum Sampler preset, cut down.
pub const DRUM_SAMPLER_TEMPLATE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Ableton MajorVersion=\"5\" MinorVersion=\"12.0_12402\">\n\t<DrumCell>\n\t\t<LastPresetRef>\n\t\t\t<Value>\n\t\t\t\t<FilePresetRef Id=\"0\"><FileRef><Path Value=\"/Core Library/Defaults/Instruments/Drum Sampler.adv\" /></FileRef></FilePresetRef>\n\t\t\t</Value>\n\t\t</LastPresetRef>\n\t\t<UserSample>\n\t\t\t<Value />\n\t\t</UserSample>\n\t\t<Voice_Gain Value=\"1\" />\n\t</DrumCell>\n</Ableton>\n";

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// A Live resources folder holding the default Drum Sampler preset; dropped with the returned directory.
pub fn live_resources() -> tempfile::TempDir {
    let folder = tempfile::Builder::new().prefix("live-resources-").tempdir().unwrap();
    let instruments = folder.path().join("Core Library").join("Defaults").join("Instruments");
    fs::create_dir_all(&instruments).unwrap();
    fs::write(instruments.join("Drum Sampler.adv"), gzip(DRUM_SAMPLER_TEMPLATE.as_bytes())).unwrap();
    folder
}
