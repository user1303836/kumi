use kumi_library::plugins::formats::{fabfilter, structure, vstpreset::VstPreset};
use kumi_library::plugins::templates;
use serde_json::json;

#[test]
fn every_fabfilter_initialized_state_and_controller_has_a_verified_layout() {
    let mut checked = 0;
    for format in structure::builtins().iter().filter(|format| format.info.family == "fabfilter") {
        let plugin = &format.info.plugin;
        let bytes = templates::initial(plugin).unwrap_or_else(|| panic!("{plugin}: missing initialized VST3"));
        let container = VstPreset::read(bytes).unwrap();
        assert!(format.has_id(&container.class_id), "{plugin}: wrong initialized class");
        let component = container.component().unwrap();
        assert!(fabfilter::is_component(component), "{plugin}: component envelope");
        let preset = fabfilter::FabFilterPreset::read(plugin, component).unwrap();
        assert_eq!(preset.write().unwrap(), component, "{plugin}: read changed bytes");
        assert_eq!(preset.to_state(component).unwrap(), component, "{plugin}: conversion changed bytes");
        if let Some(controller) = container.controller() {
            fabfilter::check_controller(plugin, controller, controller).unwrap();
            let renamed = fabfilter::rename_controller(plugin, controller, controller, "Kumi 测试").unwrap();
            fabfilter::check_controller(plugin, &renamed, controller).unwrap();
        }
        checked += 1;
    }
    assert!(checked >= 25, "all current and captured legacy FabFilter products must be represented");
}

#[test]
fn every_fabfilter_component_rejects_corrupt_headers_before_writing() {
    for format in structure::builtins().iter().filter(|format| format.info.family == "fabfilter") {
        let plugin = &format.info.plugin;
        let container = VstPreset::read(templates::initial(plugin).unwrap()).unwrap();
        let component = container.component().unwrap();
        let mut wrong_schema = component.to_vec();
        wrong_schema[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(fabfilter::FabFilterPreset::read(plugin, &wrong_schema).is_err(), "{plugin}: unknown schema accepted");
        let mut enormous_length = component.to_vec();
        enormous_length[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(fabfilter::FabFilterPreset::read(plugin, &enormous_length).is_err(), "{plugin}: oversized length accepted");
        for size in 0..12 {
            assert!(fabfilter::FabFilterPreset::read(plugin, &component[..size]).is_err(), "{plugin}: truncated header accepted");
        }
    }
}

#[test]
fn initialized_native_presets_convert_without_losing_controls_or_controller_extensions() {
    let mut checked = 0;
    for format in structure::builtins().iter().filter(|format| format.info.family == "fabfilter") {
        let plugin = &format.info.plugin;
        let path = std::path::Path::new(env!("KUMI_WORKSPACE"))
            .join("plugin-formats")
            .join(format.info.format_directory())
            .join("fixtures/init.ffp");
        let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let mut native = fabfilter::FabFilterPreset::read(plugin, &bytes).unwrap();
        assert_eq!(native.write().unwrap(), bytes, "{plugin}: native no-op changed bytes");
        let container = VstPreset::read(templates::initial(plugin).unwrap()).unwrap();
        let component = container.component().unwrap();
        assert_eq!(native.to_state(component).unwrap(), component, "{plugin}: native conversion changed initialized controls");
        if let Some(controller) = container.controller() {
            assert_eq!(native.to_controller(controller).unwrap(), controller, "{plugin}: native conversion lost controller metadata");
        }
        let before = native.to_node();
        let parameters = before.get("parameters").unwrap();
        let (path, value) = format
            .structure
            .fields
            .iter()
            .find_map(|(path, field)| {
                let name = path.strip_prefix("parameters/")?;
                let current = parameters.get(name)?.as_f64()?;
                let [low, high] = field.range.filter(|_| field.range_status == Some(structure::Status::Verified))?;
                (low < high).then(|| (path, if current == low { high } else { low }))
            })
            .unwrap_or_else(|| panic!("{plugin}: no verified native edit"));
        native.set(path, &json!(value)).unwrap();
        let edited = native.write().unwrap();
        let edited = fabfilter::FabFilterPreset::read(plugin, &edited).unwrap();
        let converted = edited.to_state(component).unwrap();
        let converted = fabfilter::FabFilterPreset::read(plugin, &converted).unwrap();
        let name = path.strip_prefix("parameters/").unwrap();
        let actual = converted.to_node().get("parameters").unwrap().get(name).unwrap().as_f64().unwrap();
        assert!((actual - value).abs() <= 1e-6_f64.max(value.abs() * 1e-6), "{plugin}/{name}: {actual} != {value}");
        let checkpoint = native.write().unwrap();
        assert!(native.set("parameters/Invented Control", &json!(0)).is_err());
        assert!(native.set(path, &json!("not a number")).is_err());
        assert_eq!(native.write().unwrap(), checkpoint, "{plugin}: refused changes mutated native data");
        checked += 1;
    }
    assert!(checked >= 25);
}

#[test]
fn older_fabfilter_component_templates_are_checked_against_their_own_layout() {
    let mut checked = 0;
    for format in structure::builtins().iter().filter(|format| format.info.family == "fabfilter") {
        for version in &format.verified.writes {
            let Some(bytes) = templates::initial_for_version(&format.info.plugin, &version.version) else { continue };
            let template = VstPreset::read(bytes).unwrap();
            let component = template.component().unwrap();
            let preset = fabfilter::FabFilterPreset::read(&format.info.plugin, component).unwrap();
            assert_eq!(preset.to_state(component).unwrap(), component);
            assert!(format.has_id(&template.class_id));
            checked += 1;
        }
    }
    assert!(checked >= 1, "the captured older Twin 3 layout needs its own creation template");
}

#[test]
fn one_native_normalized_controls_keep_the_same_physical_meaning_in_vst3() {
    let plugin = "fabfilter-one";
    let bytes = include_bytes!(concat!(env!("KUMI_WORKSPACE"), "/plugin-formats/fabfilter-one/format-1/fixtures/init-v0.ffp"));
    let template = VstPreset::read(templates::initial(plugin).unwrap()).unwrap();
    let component = template.component().unwrap();
    let mut preset = fabfilter::FabFilterPreset::read(plugin, bytes).unwrap();
    assert_eq!(preset.to_state(component).unwrap(), component, "the six controls absent from schema zero keep initialized defaults");
    preset.set("parameters/Wave Form", &json!(2)).unwrap();
    preset.set("parameters/Pitch", &json!(-6)).unwrap();
    let native = preset.write().unwrap();
    assert_eq!(f32::from_le_bytes(native[8..12].try_into().unwrap()), 2.0 / 3.0);
    assert_eq!(f32::from_le_bytes(native[12..16].try_into().unwrap()), 0.25);
    let converted = fabfilter::FabFilterPreset::read(plugin, &preset.to_state(component).unwrap()).unwrap().to_node();
    let parameters = converted.get("parameters").unwrap();
    assert_eq!(parameters.get("Wave Form").unwrap().as_f64(), Some(2.0));
    assert_eq!(parameters.get("Pitch").unwrap().as_f64(), Some(-6.0));
}
