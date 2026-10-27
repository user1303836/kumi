use kumi_library::plugins::formats::{
    juce_xml::{self, XmlState},
    minimal_audio::MinimalAudioPreset,
    pulsar_modular::PulsarModularPreset,
    structure,
    valhalla::ValhallaPreset,
    vstpreset::VstPreset,
};
use serde_json::{json, Value};

fn fixture(plugin: &str) -> VstPreset {
    let path = std::path::Path::new(env!("KUMI_WORKSPACE")).join("plugin-formats").join(plugin).join("format-1/fixtures/init.vstpreset");
    VstPreset::read(&std::fs::read(path).unwrap()).unwrap()
}

fn binary_xml(xml: &str) -> Vec<u8> {
    let mut bytes = b"VC2!".to_vec();
    bytes.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    bytes.extend_from_slice(xml.as_bytes());
    bytes.extend_from_slice(b"\0opaque private data\0\xff");
    bytes
}

#[test]
fn every_vendor_product_has_an_exact_init_roundtrip_and_a_recorded_runtime_edit() {
    let mut count = 0;
    for format in structure::builtins()
        .iter()
        .filter(|f| f.info.plugin.starts_with("valhalla-") || f.info.plugin.starts_with("minimal-") || f.info.plugin.starts_with("pulsar-"))
    {
        count += 1;
        let id = &format.info.plugin;
        let preset = fixture(id);
        assert!(format.has_id(&preset.class_id), "{id}: fixture belongs to another processor");
        assert!(format.info.ids.vst2.is_empty() && format.info.ids.au.is_empty() && format.info.ids.clap.is_empty());
        let path = std::path::Path::new(env!("KUMI_WORKSPACE")).join("plugin-formats").join(id).join("format-1/verified.json");
        let evidence: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let check = &evidence["checks"][0];
        let path = check["path"].as_str().unwrap();
        let value = &check["value"];
        let original = preset.component().unwrap();
        let (tree, changed, reread) = if id.starts_with("valhalla-") {
            let mut state = ValhallaPreset::read_state(original).unwrap();
            assert_eq!(state.write_state().unwrap(), original, "{id}");
            let tree = state.to_node();
            state.set(path, value).unwrap();
            let bytes = state.write_state().unwrap();
            let reread = ValhallaPreset::read_state(&bytes).unwrap().to_node();
            (tree, bytes, reread)
        } else if id.starts_with("minimal-") {
            let mut state = MinimalAudioPreset::read_state(original).unwrap();
            assert_eq!(state.write_state().unwrap(), original, "{id}");
            let tree = state.to_node();
            state.set(path, value).unwrap();
            let bytes = state.write_state().unwrap();
            let reread = MinimalAudioPreset::read_state(&bytes).unwrap().to_node();
            (tree, bytes, reread)
        } else {
            let mut state = PulsarModularPreset::read_state(original).unwrap();
            assert_eq!(state.write_state().unwrap(), original, "{id}");
            let tree = state.to_node();
            state.set(path, value).unwrap();
            let bytes = state.write_state().unwrap();
            let reread = PulsarModularPreset::read_state(&bytes).unwrap().to_node();
            (tree, bytes, reread)
        };
        assert!(format.structure.check(&tree).is_clean(), "{id}: fixture does not fit its structure");
        assert_ne!(changed, original, "{id}: the edit must change saved bytes");
        let field = path.strip_prefix("parameters/").unwrap();
        assert_eq!(reread.get("parameters").unwrap().get(field).unwrap().as_f64(), value.as_f64(), "{id}");
        assert!(!format.verified.writes.is_empty(), "{id}: missing runtime verification");
    }
    assert_eq!(count, 39);
}

#[test]
fn binary_xml_retains_private_tail_and_unknown_properties_after_editing() {
    let original = binary_xml("<ValhallaDelay Mix=\"0.5\" mystery=\"preserve &amp; keep\"><Unknown blob=\"data\"/></ValhallaDelay>");
    let mut state = ValhallaPreset::read_state(&original).unwrap();
    state.set("parameters/Mix", &json!(0.25)).unwrap();
    let output = state.write_state().unwrap();
    assert!(output.ends_with(b"\0opaque private data\0\xff"));
    let xml = XmlState::read(&output).unwrap();
    assert_eq!(xml.document.root.attribute("mystery"), Some("preserve & keep"));
    assert_eq!(xml.document.root.child("Unknown").unwrap().attribute("blob"), Some("data"));
    assert_eq!(xml.document.root.attribute("Mix"), Some("0.25"));
    let mut native = binary_xml("<ValhallaDelay Mix=\"0.5\"/>");
    native.extend_from_slice(b"native private payload");
    let mut native = XmlState::read(&native).unwrap();
    juce_xml::set_attribute(&mut native.document.root, "@Mix", "0.75").unwrap();
    let transplanted = native.to_state(&binary_xml("<ValhallaDelay Mix=\"0.5\"/>")).unwrap();
    assert!(transplanted.ends_with(b"native private payload"));
}

#[test]
fn binary_xml_rejects_truncation_and_inconsistent_lengths() {
    let original = binary_xml("<ValhallaDelay Mix=\"0.5\"/>");
    let declared = u32::from_le_bytes(original[4..8].try_into().unwrap()) as usize;
    for end in 0..8 + declared + 1 {
        assert!(XmlState::read(&original[..end]).is_err(), "accepted truncation at {end}");
    }
    let mut oversized = original.clone();
    oversized[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(XmlState::read(&oversized).is_err());
    let mut compatibility = fixture("valhalla-delay").component().unwrap().to_vec();
    compatibility[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(XmlState::read(&compatibility).is_err());
}

#[test]
fn minimal_audio_edits_the_authoritative_physical_value() {
    let xml = b"<ClusterDelay><App_State><Parameters><Gain unmapped_value=\"0.5\" mapped_value=\"-24\"/></Parameters><Other value=\"keep\"/></App_State></ClusterDelay>";
    let mut state = MinimalAudioPreset::read(xml).unwrap();
    state.set("parameters/Gain", &json!(-6)).unwrap();
    let output = XmlState::read(&state.write().unwrap()).unwrap();
    let gain = output.document.root.child("App_State").unwrap().child("Parameters").unwrap().child("Gain").unwrap();
    assert_eq!(gain.attribute("mapped_value"), Some("-6"));
    assert_eq!(gain.attribute("unmapped_value"), Some("0.5"));
    assert!(state.set("state/App_State/Parameters/Gain/@unmapped_value", &json!(0.8)).is_err());
    assert!(state.set_parameter("invented", 0.5).is_err());
    assert!(state.set_parameter("Gain", f64::NAN).is_err());
}

#[test]
fn pulsar_rejects_obsolete_control_ids_and_preserves_them() {
    let preset = fixture("pulsar-p11-abyss");
    let mut state = PulsarModularPreset::read_state(preset.component().unwrap()).unwrap();
    let original = XmlState::read(preset.component().unwrap()).unwrap();
    assert!(state.set_parameter("parammix", 0.3).is_err());
    state.set_parameter("mix", 0.26).unwrap();
    let updated = XmlState::read(&state.write_state().unwrap()).unwrap();
    let legacy =
        |state: &XmlState| state.document.root.children.iter().find(|element| element.attribute("id") == Some("parammix")).cloned();
    assert_eq!(legacy(&original), legacy(&updated));
}

#[test]
fn explicit_xml_paths_refuse_ambiguous_repeated_children_and_new_attributes() {
    let mut document = XmlState::read(b"<Root><Item value=\"1\"/><Item value=\"2\"/></Root>").unwrap();
    assert!(juce_xml::set_attribute(&mut document.document.root, "Item/@value", "3").is_err());
    assert!(juce_xml::set_attribute(&mut document.document.root, "Item[0]/@new", "3").is_err());
    juce_xml::set_attribute(&mut document.document.root, "Item[1]/@value", "3").unwrap();
    assert_eq!(document.document.root.children[0].attribute("value"), Some("1"));
    assert_eq!(document.document.root.children[1].attribute("value"), Some("3"));
    assert!(ValhallaPreset::read(b"<Unrelated Mix=\"0.5\"/>").is_err());
    let mut valhalla = ValhallaPreset::read(b"<ValhallaDelay Mix=\"0.5\"/>").unwrap();
    assert!(valhalla.set_parameter("Mix", 2.0).is_err());
    assert!(valhalla.set_parameter("nonexistent", 0.2).is_err());
}

#[test]
fn native_xml_converts_to_the_matching_product_component_envelope() {
    let init = fixture("valhalla-delay");
    let initialized = ValhallaPreset::read_state(init.component().unwrap()).unwrap();
    let mut native = ValhallaPreset::read(&initialized.write_preset()).unwrap();
    native.set_parameter("Mode", 0.125).unwrap();
    let component = native.to_state(init.component().unwrap()).unwrap();
    assert!(component.starts_with(b"VstW"));
    assert_eq!(ValhallaPreset::read_state(&component).unwrap().parameters().find(|(key, _)| *key == "Mode").unwrap().1, 0.125);
    let shimmer = fixture("valhalla-shimmer");
    let freq_echo = fixture("valhalla-freq-echo");
    let shimmer = ValhallaPreset::read_state(shimmer.component().unwrap()).unwrap();
    assert!(shimmer.to_state(freq_echo.component().unwrap()).is_err());
}
