use kumi_library::plugins::formats::tree::Node;
use kumi_library::plugins::formats::vstpreset::VstPreset;
use kumi_library::plugins::formats::{softube, superior_drummer, zynaptiq};
use serde_json::json;

fn fixture(plugin: &str, file: &str) -> Vec<u8> {
    std::fs::read(kumi_testkit::workspace_path("plugin-formats").join(plugin).join("format-1/fixtures").join(file)).unwrap()
}

#[test]
fn zynaptiq_component_and_native_presets_share_parameter_values() {
    for (plugin, parameter, value) in [("zynaptiq-punch-1", 4, 3.0), ("zynaptiq-unmix-drums", 1, 0.7)] {
        let wrapper = VstPreset::read(&fixture(plugin, "init.vstpreset")).unwrap();
        let mut component = zynaptiq::State::read(wrapper.component().unwrap()).unwrap();
        let native = zynaptiq::Preset::read(&fixture(plugin, "init.preset")).unwrap();
        assert_eq!(component.preset, native);
        let unknown_before = component.preset.document.root.child("inputCustomProperties").cloned();
        component.set(&format!("inputParams/@p{parameter}"), &json!(value)).unwrap();
        let written = component.write().unwrap();
        assert_eq!(u32::from_le_bytes(written[..4].try_into().unwrap()) as usize, written.len() - 4);
        let reread = zynaptiq::State::read(&written).unwrap();
        assert_eq!(reread.preset.parameter(parameter), Some(value));
        assert_eq!(reread.preset.document.root.child("inputCustomProperties"), unknown_before.as_ref());
        assert_eq!(zynaptiq::Preset::read(&reread.preset.write().unwrap()).unwrap(), reread.preset);
    }
}

#[test]
fn zynaptiq_rejects_bad_lengths_unknown_parameters_and_nonfinite_numbers() {
    let wrapper = VstPreset::read(&fixture("zynaptiq-punch-1", "init.vstpreset")).unwrap();
    let bytes = wrapper.component().unwrap();
    for length in 0..bytes.len() {
        assert!(zynaptiq::State::read(&bytes[..length]).is_err());
    }
    let mut state = zynaptiq::State::read(bytes).unwrap();
    let before = state.clone();
    assert!(state.preset.set_parameter(99999, 0.5).is_err());
    assert!(state.preset.set_parameter(4, f64::NAN).is_err());
    assert!(state.set("inputParams/@count", &json!(2)).is_err());
    assert_eq!(state, before);
    let mut invalid = bytes.to_vec();
    invalid[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(zynaptiq::State::read(&invalid).is_err());
    assert!(zynaptiq::Preset::read(b"<!DOCTYPE Preset><Preset version=\"4\"/>").is_err());
    assert!(zynaptiq::Preset::read(b"<Preset version=\"4\"><inputParams count=\"0\"/><inputParams count=\"0\"/></Preset>").is_err());
    assert!(zynaptiq::Preset::read(b"<Preset version=\"4\"><inputParams count=\"0\" count=\"1\"/></Preset>").is_err());
    let excessive = b"<Preset version=\"4\"><inputParams count=\"1000000\" p0=\"0\"/></Preset>";
    assert!(zynaptiq::Preset::read(excessive).unwrap_err().to_string().contains("too many parameters"));
    assert!(zynaptiq::Preset::read(b"<Preset version=\"4\"><inputParams count=\"2\" p0=\"0\" p01=\"0\"/></Preset>").is_err());
    assert!(zynaptiq::Preset::read(
        b"<Preset version=\"4\"><!-- <inputParams count='9999999'/> --><inputParams count=\"1\" p0=\"0\"/></Preset>"
    )
    .is_ok());
}

#[test]
fn softube_edits_only_the_addressed_preset_and_keeps_shared_data() {
    let mut preset =
        softube::Preset::create("test", "Synthetic effect", "1.0.0", "Kumi", &[(2, 0.25, "Gain".into()), (9, 0.75, "Mix".into())]).unwrap();
    preset.document["SharedData"] = json!({"opaque":"do not change"});
    preset.document["PresetList"][0]["future"] = json!({"nested":[1,2,3]});
    let second = preset.document["PresetList"][0].clone();
    preset.document["PresetList"].as_array_mut().unwrap().push(second.clone());
    preset.set("presets/0/parameters/2", &json!(0.5)).unwrap();
    preset.set("presets/0/name", &json!("Kumi revised")).unwrap();
    let read = softube::Preset::read(&preset.write().unwrap()).unwrap();
    assert_eq!(read.document["PresetList"][0]["ParameterData"]["Parameters"][0][1], 0.5);
    assert_eq!(read.document["PresetList"][1], second);
    assert_eq!(read.document["SharedData"], json!({"opaque":"do not change"}));
    assert_eq!(read.document["PresetList"][0]["future"], json!({"nested":[1,2,3]}));
    assert!(preset.set("presets/0/parameters/2", &json!(1.01)).is_err());
    assert!(preset.set("presets/0/parameters/1234", &json!(0.5)).is_err());
    assert!(preset.set_parameter(0, 2, f64::INFINITY).is_err());
}

#[test]
fn softube_rejects_ambiguous_or_partial_parameter_sets() {
    let mut preset = softube::Preset::create("test", "Synthetic effect", "1", "Kumi", &[(0, 0.5, "Gain".into())]).unwrap();
    preset.document["PresetList"][0]["ParameterData"]["Parameters"].as_array_mut().unwrap().push(json!([0, 0.2, "Duplicate"]));
    assert!(preset.write().is_err());
    preset.document["PresetList"][0]["ParameterData"]["Type"] = json!("Partial");
    assert!(preset.write().is_err());
}

#[test]
fn native_template_copies_retain_source_state_and_new_softube_identity() {
    let original = softube::Preset::create("test", "Synthetic effect", "1", "Source", &[(0, 0.25, "Gain".into())]).unwrap();
    let mut template = original.clone();
    template.document["SharedData"] = json!({"externalRef":"retained"});
    template.document["PresetList"].as_array_mut().unwrap().push(original.document["PresetList"][0].clone());
    let mut created = softube::Preset::from_template(&template, 1, "New native preset").unwrap();
    assert_eq!(created.document["PresetList"].as_array().unwrap().len(), 1);
    assert_ne!(created.document["PresetList"][0]["UUID"], original.document["PresetList"][0]["UUID"]);
    assert_eq!(created.document["SharedData"], template.document["SharedData"]);
    created.set_parameter(0, 0, 0.75).unwrap();
    assert_eq!(template.document["PresetList"][1]["ParameterData"]["Parameters"][0][1], 0.25);
    assert_eq!(softube::Preset::read(&created.write().unwrap()).unwrap(), created);

    let template = zynaptiq::Preset::read(&fixture("zynaptiq-punch-1", "init.preset")).unwrap();
    let mut created = zynaptiq::Preset::from_template(&template, "New native preset").unwrap();
    assert_eq!(created.document.root.attribute("progName"), Some("New native preset"));
    assert_eq!(created.document.root.child("inputParams"), template.document.root.child("inputParams"));
    created.set_parameter(4, 2.0).unwrap();
    assert_ne!(created.parameter(4), template.parameter(4));
    assert_eq!(zynaptiq::Preset::read(&created.write().unwrap()).unwrap(), created);
}

#[test]
fn superior_creates_sampler_state_inside_its_vst3_container() {
    let mut wrapper = VstPreset::read(&fixture("superior-drummer-3", "empty.vstpreset")).unwrap();
    let mut state = superior_drummer::State::read(wrapper.component().unwrap()).unwrap();
    assert!(state.preset.is_none());
    assert_eq!(state.version(), "3.4.1");
    assert_eq!(state.write().unwrap(), wrapper.component().unwrap());
    state.preset = Some(superior_drummer::Preset::create("3.4.1").unwrap());
    assert_eq!(state.preset.as_ref().unwrap().version().as_deref(), Some("3.4.1"));
    assert!(state.set("PlugVersion[0]", &json!("3.5.0")).is_err());
    state.set("SAMPLER[0]/HQ[0]/mastervol[0]", &json!(0.5)).unwrap();
    let component = state.write().unwrap();
    let reread = superior_drummer::State::read(&component).unwrap();
    assert_eq!(reread.preset, state.preset);
    assert_eq!(reread.to_node().get("SAMPLER[0]/HQ[0]/mastervol[0]"), Some(&Node::Float(0.5)));
    wrapper.chunks.iter_mut().find(|(id, _)| id == "Comp").unwrap().1 = component;
    assert_eq!(VstPreset::read(&wrapper.write()).unwrap().class_id, "565354534433617375706572696F7220");

    let mut old = wrapper.component().unwrap().to_vec();
    let bank = 8 + u32::from_be_bytes(old[4..8].try_into().unwrap()) as usize;
    old[bank + 20..bank + 24].copy_from_slice(&0x0003_0300u32.to_be_bytes());
    let old = superior_drummer::State::read(&old).unwrap();
    assert_eq!(old.version(), "3.3.0");
    assert_eq!(old.preset.unwrap().version().as_deref(), Some("3.4.1"));
    assert!(superior_drummer::Preset::read(b"PlugVersion 3.4.1\nPlugVersion 3.3.0\nSAMPLER {\n}\n").is_err());
}

#[test]
fn superior_numeric_edits_preserve_other_sections_and_refuse_text_changes() {
    let bytes = b"PlugVersion 3.4.1\r\nSAMPLER {\r\nHQ {\r\nmastervol 1\r\n}\r\nmic {\r\nname \"Room One\"\r\nlevels 1 2\r\n}\r\nmic {\r\nname \"Room Two\"\r\n}\r\n}\r\n";
    let mut preset = superior_drummer::Preset::read(bytes).unwrap();
    assert_eq!(preset.write(), bytes);
    for value in ["My \"room\"", r"\\host\share\sample.wav", r"C:\Samples\kick.wav", "/mnt/library/snare.wav"] {
        let before = preset.clone();
        assert!(preset.set("SAMPLER[0]/mic[0]/name[0]", &json!(value)).unwrap_err().to_string().contains("read-only"));
        assert_eq!(preset, before);
    }
    preset.set("SAMPLER[0]/mic[0]/levels[0]", &json!([0.25, 0.75])).unwrap();
    let written = String::from_utf8(preset.write()).unwrap();
    assert!(written.contains("name \"Room One\"\r\n"));
    assert!(written.contains("name \"Room Two\"\r\n"));
    assert!(written.contains("mastervol 1\r\n"));
    assert!(preset.set("SAMPLER[0]/mic[0]/levels[0]", &json!(0.5)).is_err());
    assert!(preset.set("SAMPLER[0]/mic[0]/name[0]", &json!("bad\nrecord")).is_err());
    assert!(preset.set("SAMPLER[0]/missing[0]", &json!(0.5)).is_err());
    assert!(superior_drummer::Preset::read(b"SAMPLER {\nname \"unfinished\n}\n").is_err());
    assert!(superior_drummer::Preset::read(b"SAMPLER {\n").is_err());
}

#[test]
fn superior_section_arguments_quoted_braces_and_bare_text_remain_read_only() {
    let bytes = b"PlugVersion 3.4.1\nSAMPLER {\ninstbox 12 \"Kick { 2 }\" {\nname \"123\"\nlevel 1\nmode kick\n}\n}\n";
    let mut preset = superior_drummer::Preset::read(bytes).unwrap();
    assert_eq!(
        preset.to_node().get("SAMPLER[0]/instbox[0]/$args"),
        Some(&Node::List(vec![Node::Float(12.0), Node::Text("Kick { 2 }".into())]))
    );
    assert_eq!(preset.to_node().get("SAMPLER[0]/instbox[0]/name[0]"), Some(&Node::Text("123".into())));
    assert!(preset.set("SAMPLER[0]/instbox[0]/$args", &json!([15, "Snare { main }"])).is_err());
    assert!(preset.set("SAMPLER[0]/instbox[0]/$args", &json!(["15", "Snare"])).is_err());
    assert!(preset.set("SAMPLER[0]/instbox[0]/name[0]", &json!(123)).is_err());
    assert!(preset.set("SAMPLER[0]/instbox[0]/level[0]", &json!("1")).is_err());
    assert!(preset.set("SAMPLER[0]/instbox[0]/mode[0]", &json!("snare")).is_err());
    assert!(preset.set("SAMPLER[0]/instbox[0]/mode[0]", &json!("two tokens")).is_err());
    assert!(preset.set("SAMPLER[0]/instbox[0]/mode[0]", &json!("123")).is_err());
    let reread = superior_drummer::Preset::read(&preset.write()).unwrap();
    assert_eq!(
        reread.to_node().get("SAMPLER[0]/instbox[0]/$args"),
        Some(&Node::List(vec![Node::Float(12.0), Node::Text("Kick { 2 }".into())]))
    );
    assert_eq!(preset.write(), bytes);
}

#[test]
fn superior_rejects_path_amplification_within_input_and_token_limits() {
    // Both inputs are small and every individual path is below 4096 bytes.
    // Materializing the repeated parent path without an aggregate budget
    // would nevertheless retain more than 60 MiB of generated path strings.
    for repeated in ["a 1\n".repeat(32_000), "a 1 {\n}\n".repeat(16_000)] {
        let text = format!("PlugVersion 3.4.1\nSAMPLER {{\n{} {{\n{repeated}}}\n}}\n", "s".repeat(3900));
        assert!(text.len() < 140_000);
        let error = superior_drummer::Preset::read(text.as_bytes()).unwrap_err();
        assert!(error.to_string().contains("total record path budget exceeded"), "{error}");
    }

    // An ordinary smaller document with the same long parent remains usable.
    let section = "s".repeat(3900);
    let text = format!("PlugVersion 3.4.1\nSAMPLER {{\n{section} {{\n{}}}\n}}\n", "a 1\n".repeat(100));
    let mut preset = superior_drummer::Preset::read(text.as_bytes()).unwrap();
    let path = format!("SAMPLER[0]/{section}[0]/a[99]");
    preset.set(&path, &json!(0.5)).unwrap();
    assert_eq!(preset.to_node().get(&path), Some(&Node::Float(0.5)));
}

#[test]
fn softube_observed_normalization_does_not_open_numeric_write_ranges() {
    use kumi_library::plugins::formats::structure::{self, Status};

    let mut ranges = 0;
    for plugin in [
        "softube-tape",
        "softube-tape-multi-track",
        "softube-weiss-compressor-limiter",
        "softube-weiss-ds1-mk3",
        "softube-weiss-deess",
        "softube-weiss-mm-1-mastering-maximizer",
    ] {
        let format = structure::builtin(plugin).unwrap();
        for (path, field) in &format.structure.fields {
            if path.starts_with("presets[]/parameters/") {
                ranges += 1;
                assert_eq!(field.range_status, Some(Status::Guessed), "{plugin}/{path}");
                assert!(structure::validate_number(plugin, path, 0.5).is_err(), "{plugin}/{path}");
            }
        }
    }
    assert_eq!(ranges, 194);
}
