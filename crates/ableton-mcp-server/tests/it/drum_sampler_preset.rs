#[path = "support/drum_sampler.rs"]
mod drum_sampler;

use std::io::Read;

use ableton_mcp_server::drum_sampler_preset::{drum_sampler_preset, find_drum_sampler_template, DrumSamplerSample, DrumSamplerTemplate};
use drum_sampler::{live_resources, DRUM_SAMPLER_TEMPLATE as TEMPLATE};
use flate2::read::MultiGzDecoder;
use regex::Regex;

fn gunzip(bytes: &[u8]) -> String {
    let mut xml = String::new();
    MultiGzDecoder::new(bytes).read_to_string(&mut xml).unwrap();
    xml
}

#[test]
fn a_drum_sampler_preset_carries_the_sample_from_lives_own_default_preset() {
    let resources = live_resources();
    let folder = resources.path().to_string_lossy().into_owned();
    let join =
        |parts: &[&str]| parts.iter().fold(resources.path().to_path_buf(), |path, part| path.join(part)).to_string_lossy().into_owned();
    let template = find_drum_sampler_template(&[join(&["missing"]), folder.clone()]).unwrap();
    assert_eq!(template.builtin_device_path, join(&["Builtin", "Devices", "Instruments", "Drum Sampler"]), "the first Live that has one");
    let path = "/staged/Kick \"Big\" & Sub $& 808.wav";
    let xml = gunzip(
        &drum_sampler_preset(&template, &DrumSamplerSample { path: path.to_string(), size: 1234.5, modified_seconds: 1700000000.7 })
            .unwrap(),
    );
    assert!(Regex::new(r#"<SampleRef Id="0">"#).unwrap().is_match(&xml));
    assert!(
        xml.contains(r#"<Path Value="/staged/Kick &quot;Big&quot; &amp; Sub $&amp; 808.wav" />"#),
        "the path is escaped, and a $ stays a $"
    );
    assert!(Regex::new(r#"<OriginalFileSize Value="1234" />"#).unwrap().is_match(&xml));
    assert!(Regex::new(r#"<LastModDate Value="1700000000" />"#).unwrap().is_match(&xml));
    assert!(
        Regex::new(r#"(?s)<AbletonDefaultPresetRef Id="0">.*<DeviceId Name="DrumCell" />"#).unwrap().is_match(&xml),
        "it says it came from the built-in Drum Sampler"
    );
    assert!(!Regex::new("FilePresetRef").unwrap().is_match(&xml), "not from the default preset's file");
    assert!(Regex::new(r#"<Voice_Gain Value="1" />"#).unwrap().is_match(&xml), "the rest as Live wrote it");
    let unknown_shape = DrumSamplerTemplate { xml: TEMPLATE.replacen("<Value />", "<Value><SampleRef /></Value>", 1), ..template.clone() };
    let error =
        drum_sampler_preset(&unknown_shape, &DrumSamplerSample { path: path.to_string(), size: 1.0, modified_seconds: 1.0 }).unwrap_err();
    assert!(error.to_string().contains("shape the bridge doesn't know"), "{error}");
    assert_eq!(find_drum_sampler_template(&[join(&["missing"])]), None);
}
